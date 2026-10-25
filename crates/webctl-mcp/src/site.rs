//! resolve_site 工具（0.2 / ADR-0006）：站点名 -> 入口候选。
//!
//! 刻意放在独立模块而不是塞进 dispatch.rs：派发表已经 900+ 行，仓库规则要求
//! 单文件 < 1000 行；这里只放这一个工具的实现与用例。
//!
//! 通道：
//! - L1（本地、不联网）：内置映射表 + 域名直推；
//! - L3（联网、过 politeness）：Wikidata P856 官方网站（offline 模式下不尝试）；
//! - L4（联网、过 politeness）：候选域名 robots.txt 的 Sitemap 声明（只报告，不提升置信度）；
//! - L5（联网、过 politeness）：**显式 opt-in** 的 SERP 兜底（参数 allow_serp_fallback=true），
//!   默认关闭；结果按不可靠来源处理，被拦即如实报 BLOCKED。
//!
//! 诚实性（ADR-0006）：
//! - 永远给**候选列表**（绝不单选），每条带 url/score/evidence/channel；
//! - 内置映射（95 分）或 Wikidata P856（90 分）才算"可靠定位"；只有直推（≤70 分）
//!   或零命中时**显式降级**并给 actionable（reason/detected/next_steps）；
//! - L3 失败/被 robots 拦下时**如实说卡在哪一步**（不静默当成"没找到"）。

use serde_json::{Value, json};
use webctl_core::{Actionable, Envelope, ErrorCode, Trust};
use webctl_web::politeness::{PoliteFetcher, PolitenessConfig};
use webctl_web::resolve_site::serp::{self, SerpConfig, SerpFailure};
use webctl_web::resolve_site::site_search::{
    self, SiteSearchConfig, SiteSearchReport, SiteSearchStatus,
};
use webctl_web::resolve_site::sitemap::{self, SitemapReport, SitemapStatus};
use webctl_web::resolve_site::wikidata::{self, L3Config, L3Failure};
use webctl_web::resolve_site::{Candidate, merge, resolve_l1};

use crate::context::McpContext;
use crate::dispatch::{fail, parse, to_value};
use crate::params::ResolveSiteParams;

/// 可靠定位阈值：内置映射 95、Wikidata P856 90；直推最高 70。60 分及以上算可靠。
const CONFIDENT_SCORE: u8 = 60;

/// 默认候选数。
const DEFAULT_MAX_CANDIDATES: usize = 5;
/// 候选数上限（避免调用方要 1000 条）。
const MAX_CANDIDATES_CAP: usize = 10;
/// L4 最多探测几个候选的 robots.txt（每个候选一次请求，给工具预算留余量）。
const SITEMAP_PROBE_LIMIT: usize = 2;

/// 一次定位尝试的汇总（把"跑了哪些层/哪层失败"显式带出来）。
struct Resolution {
    candidates: Vec<Candidate>,
    layers: Vec<&'static str>,
    l3_failure: Option<L3Failure>,
    serp_failure: Option<SerpFailure>,
    sitemaps: Vec<SitemapReport>,
    site_search: Vec<SiteSearchReport>,
}

/// 依次跑 L1；仍不可靠时跑 L2（站点内建搜索）→ L3（Wikidata）→ L4（sitemap 声明）；
/// 调用方**显式**开启时才跑 L5（SERP 兜底）。
async fn resolve_with_layers(
    ctx: &McpContext,
    site: &str,
    max_candidates: usize,
    allow_serp: bool,
) -> Resolution {
    let l1 = resolve_l1(site, max_candidates);
    let mut layers = vec!["l1"];
    let mut candidates = l1;
    let mut l3_failure = None;
    let mut serp_failure = None;
    let mut sitemaps = Vec::new();
    let mut site_search = Vec::new();
    // offline 模式不联网：L1 之外一律不尝试（并在降级原因里说清楚）。
    if best_score(&candidates) < CONFIDENT_SCORE && !ctx.config.offline {
        let fetcher = PoliteFetcher::new(PolitenessConfig::default(), ctx.fetch_options());
        // L2 站点内建搜索：最多探测 2 个模板，逐站点如实记录。
        if best_score(&candidates) < CONFIDENT_SCORE {
            let outcome = site_search::search(site, &SiteSearchConfig::default(), &fetcher).await;
            layers.push("site-search");
            candidates = merge(vec![candidates, outcome.candidates], max_candidates);
            site_search = outcome.reports;
        }
        // L3 Wikidata P856（只有还没可靠时才跑，命中即停）。
        let outcome = wikidata::resolve(site, &L3Config::default(), &fetcher).await;
        l3_failure = outcome.failure;
        if best_score(&candidates) < CONFIDENT_SCORE {
            layers.push("public-api");
            candidates = merge(vec![candidates, outcome.candidates], max_candidates);
        }
        // L4 只对**已有候选**做声明探测；命中 P856 后已可靠，就不再多发请求。
        if best_score(&candidates) < CONFIDENT_SCORE && !candidates.is_empty() {
            layers.push("sitemap");
            sitemaps = sitemap::probe_top(&candidates, SITEMAP_PROBE_LIMIT, &fetcher).await;
        }
        // L5 是显式 opt-in 的最后一招：调用方没写 allow_serp_fallback=true 就绝不触发。
        if allow_serp && best_score(&candidates) < CONFIDENT_SCORE {
            eprintln!(
                "webctl: resolve_site 已启用 L5 兜底（allow_serp_fallback=true，query={site:?}）"
            );
            let outcome = serp::search(site, &SerpConfig::default(), &fetcher).await;
            layers.push("serp-fallback");
            candidates = merge(vec![candidates, outcome.candidates], max_candidates);
            serp_failure = outcome.failure;
        }
    }
    Resolution {
        candidates,
        layers,
        l3_failure,
        serp_failure,
        sitemaps,
        site_search,
    }
}

/// 候选里的最高置信分（空列表为 0）。
fn best_score(candidates: &[Candidate]) -> u8 {
    candidates
        .iter()
        .map(|candidate| candidate.score)
        .max()
        .unwrap_or(0)
}

/// 未达到可靠阈值时，给出（原因码, 证据）。证据必须能被调用方复核。
fn classify(
    candidates: &[Candidate],
    site: &str,
    offline: bool,
    l3_failure: Option<&L3Failure>,
    serp_failure: Option<&SerpFailure>,
) -> (String, Vec<String>) {
    let best = best_score(candidates);
    let mut detected = Vec::new();
    if let Some(failure) = l3_failure {
        detected.push(failure.describe());
        if let Some(serp) = serp_failure {
            detected.push(serp.describe());
        }
        detected.push(format!(
            "L1 也没有可靠命中（最高分 {best} < {CONFIDENT_SCORE}）"
        ));
        return ("resolve-site-l3-failed".to_owned(), detected);
    }
    // 可达性说明：L5 只在 L3 之后运行，所以 L3 有失败时上面的分支会先命中；
    // 这个分支覆盖"L3 成功/无失败、但 L5 失败"（以及 L3 未被尝试的调用形态），并有用例钉住。
    if let Some(failure) = serp_failure {
        detected.push(failure.describe());
        detected.push(format!(
            "L1/L3/L4 也没有可靠命中（最高分 {best} < {CONFIDENT_SCORE}）"
        ));
        return ("resolve-site-serp-failed".to_owned(), detected);
    }
    if offline {
        detected.push("offline 模式已启用：未尝试 L3（Wikidata P856）".to_owned());
        detected.push(format!(
            "L1 也没有可靠命中（最高分 {best} < {CONFIDENT_SCORE}）"
        ));
        return ("resolve-site-offline".to_owned(), detected);
    }
    if candidates.is_empty() {
        detected.push(format!("内置映射表未收录「{site}」"));
        detected.push("名称不是 ASCII 域名候选，未做任何猜测".to_owned());
        return ("resolve-site-no-match".to_owned(), detected);
    }
    detected.push(format!(
        "仅命中直推候选，最高置信分 {best} < {CONFIDENT_SCORE}"
    ));
    detected.push("直推按常见 TLD 拼装，域名可能属于他人或根本不存在".to_owned());
    ("resolve-site-low-confidence".to_owned(), detected)
}

/// 站点名 -> 入口候选。
pub(crate) async fn tool_resolve_site(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<ResolveSiteParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    let site = params.site.trim();
    if site.is_empty() {
        return fail(ErrorCode::Parse, "site 不能为空");
    }
    if site.contains("://") {
        return fail(
            ErrorCode::BadUrl,
            format!("resolve_site 只接受站点名，不接受 URL：{site}（已有 URL 请直接调用 fetch）"),
        );
    }
    let max_candidates = params
        .max_candidates
        .unwrap_or(DEFAULT_MAX_CANDIDATES)
        .clamp(1, MAX_CANDIDATES_CAP);
    // L5 默认关：只有显式 true 才开（参数缺省 / false 都不触发）。
    let allow_serp = params.allow_serp_fallback.unwrap_or(false);
    let resolution = resolve_with_layers(ctx, site, max_candidates, allow_serp).await;
    let best = best_score(&resolution.candidates);
    let confident = best >= CONFIDENT_SCORE;
    let data = json!({
        "query": site,
        "candidates": resolution.candidates,
        "confidence": if confident { "high" } else if resolution.candidates.is_empty() { "none" } else { "low" },
        "layers": resolution.layers,
        "sitemaps": resolution.sitemaps,
        "site_search": resolution.site_search,
        "serp_fallback": allow_serp,
        "channel_note": "builtin=内置映射表（可靠）；public-api=Wikidata P856 官方网站声明（可靠）；direct=按常见 TLD 直推（猜测，需自行验证）；site-search=站点内建搜索（逐站点报告状态，只收名称匹配的外部链接）；sitemaps=候选域名 robots.txt 的 Sitemap 声明（只证明站点在运营，不证明是你要的那个站）；serp-fallback=显式开启的 SERP 兜底（最不可靠，低分且不跨阈值）；serp_fallback 字段只表示『调用方请求开启』，offline 时不会执行",
    });
    if confident {
        // 本地推理 + 结构化声明，不是页面内容。
        return to_value(Envelope::ok(data, Trust::TrustedLocal, 0));
    }
    // A3：低置信/零命中是降级，不伪装成"定位成功"。
    // Envelope::degraded 固定 Trust::UntrustedPageContent —— 这里**保守**标记为不可信内容。
    let (reason, mut detected) = classify(
        &resolution.candidates,
        site,
        ctx.config.offline,
        resolution.l3_failure.as_ref(),
        resolution.serp_failure.as_ref(),
    );
    if allow_serp && ctx.config.offline {
        detected.push("offline 模式已启用：请求了 L5 兜底也不会联网执行".to_owned());
    }
    // L4 的声明只作为**证据**补充，不改变 reason（它证明"站点在运营"，不证明"就是它"）。
    for report in &resolution.sitemaps {
        match report.status {
            SitemapStatus::Declared => detected.push(format!(
                "候选 {} 的 robots.txt 声明了 {} 条 sitemap（站点在运营，但名称↔站点对应关系仍未验证）",
                report.candidate_url,
                report.sitemaps.len()
            )),
            SitemapStatus::None => detected.push(format!(
                "候选 {} 的 robots.txt 没有 Sitemap 声明（不等于不是站点）",
                report.candidate_url
            )),
            SitemapStatus::Unavailable => detected.push(format!(
                "候选 {} 的 robots.txt 拿不到（可能 404 或屏蔽爬虫），无法据此判断",
                report.candidate_url
            )),
        }
    }
    // L2 的逐站点结果也是证据：说清哪个站被拦、哪个站没匹配到，而不是一句"没找到"。
    for report in &resolution.site_search {
        let label = match report.status {
            SiteSearchStatus::Hit => "命中",
            SiteSearchStatus::Empty => "无匹配",
            SiteSearchStatus::Unavailable => "不可用",
        };
        detected.push(format!(
            "站点内建搜索 {}（{}）：{}（{} 条候选）",
            report.site, label, report.note, report.candidates
        ));
    }
    to_value(
        Envelope::degraded(data, "resolve-site", ErrorCode::Degraded, 0).with_actionable(
            Actionable {
                reason,
                detected,
                next_steps: vec!["fetch".to_owned(), "search".to_owned()],
            },
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use webctl_core::Config;

    fn offline_ctx(tag: &str) -> McpContext {
        McpContext::unbound(Config {
            state_dir: std::env::temp_dir().join(format!("webctl-site-{tag}")),
            offline: true,
            ..Config::default()
        })
    }

    /// 内置命中：高分 + 可信 + 没有 actionable（干净的成功信封），且没有联网层。
    #[tokio::test]
    async fn builtin_hit_is_confident_and_not_degraded() {
        let value = tool_resolve_site(&offline_ctx("builtin"), &json!({"site": "知乎"})).await;
        assert_eq!(value["ok"], true);
        assert_eq!(value["code"], "OK");
        assert_eq!(value["data"]["confidence"], "high");
        assert_eq!(value["data"]["candidates"][0]["url"], "https://zhihu.com");
        assert_eq!(value["data"]["candidates"][0]["channel"], "builtin");
        assert_eq!(value["data"]["candidates"][0]["score"], 95);
        assert_eq!(value["data"]["layers"], json!(["l1"]));
        assert!(
            value["meta"]["actionable"].is_null(),
            "可靠定位不该带 actionable：{value}"
        );
    }

    /// 离线 + 只有直推：显式降级，并如实说"是 offline 没走 L3"。
    #[tokio::test]
    async fn direct_only_in_offline_mode_degrades_and_says_so() {
        let value = tool_resolve_site(&offline_ctx("direct"), &json!({"site": "example"})).await;
        assert_eq!(value["ok"], true);
        assert_eq!(value["code"], "DEGRADED");
        assert_eq!(value["data"]["confidence"], "low");
        assert_eq!(value["data"]["candidates"][0]["url"], "https://example.com");
        assert_eq!(value["data"]["layers"], json!(["l1"]));
        assert_eq!(value["meta"]["degraded_from"], "resolve-site");
        let actionable = &value["meta"]["actionable"];
        assert_eq!(actionable["reason"], "resolve-site-offline");
        let detected = actionable["detected"].as_array().expect("detected");
        assert!(
            detected
                .iter()
                .any(|item| item.as_str().is_some_and(|text| text.contains("offline"))),
            "证据里要说清 offline：{actionable}"
        );
        assert_eq!(actionable["next_steps"], json!(["fetch", "search"]));
    }

    /// 不认识的名称：空候选（不猜）+ 显式降级。
    #[tokio::test]
    async fn unknown_name_returns_empty_and_degraded() {
        let value = tool_resolve_site(
            &offline_ctx("unknown"),
            &json!({"site": "某个不知名的中文站"}),
        )
        .await;
        assert_eq!(value["ok"], true);
        assert_eq!(value["code"], "DEGRADED");
        assert_eq!(value["data"]["confidence"], "none");
        assert_eq!(value["data"]["candidates"], json!([]));
    }

    /// 传 URL 是误用：直接 BAD_URL，并指向正确的工具。
    #[tokio::test]
    async fn url_argument_is_bad_url() {
        let value =
            tool_resolve_site(&offline_ctx("url"), &json!({"site": "https://zhihu.com"})).await;
        assert_eq!(value["ok"], false);
        assert_eq!(value["code"], "BAD_URL");
        assert!(
            value["error"]
                .as_str()
                .is_some_and(|text| text.contains("fetch")),
            "错误要指出改用哪个工具：{value}"
        );
    }

    /// 空名字是参数错误，不是"没找到"。
    #[tokio::test]
    async fn empty_site_is_parse_error() {
        let value = tool_resolve_site(&offline_ctx("empty"), &json!({"site": "   "})).await;
        assert_eq!(value["ok"], false);
        assert_eq!(value["code"], "PARSE");
    }

    /// max_candidates 必须被夹到 [1, 10]：要 100 条不能真给 100 条。
    #[tokio::test]
    async fn max_candidates_is_clamped() {
        let many = tool_resolve_site(
            &offline_ctx("many"),
            &json!({"site": "example", "max_candidates": 100}),
        )
        .await;
        let count = many["data"]["candidates"]
            .as_array()
            .expect("candidates")
            .len();
        assert!(count <= MAX_CANDIDATES_CAP, "上限 10 条：{count}");
        let one = tool_resolve_site(
            &offline_ctx("one"),
            &json!({"site": "example", "max_candidates": 0}),
        )
        .await;
        assert_eq!(
            one["data"]["candidates"]
                .as_array()
                .expect("candidates")
                .len(),
            1,
            "0 也要至少 1 条"
        );
    }

    // ---- classify：原因码与证据（不依赖网络，直接钉住口径） ----

    fn candidate(score: u8) -> Candidate {
        Candidate {
            url: "https://example.com".to_owned(),
            score,
            evidence: "test".to_owned(),
            channel: "direct",
        }
    }

    #[test]
    fn classify_l3_failure_wins_over_other_reasons() {
        let failure = L3Failure::SearchUnavailable {
            code: "BLOCKED".to_owned(),
            message: "robots.txt disallows https://example.com/api".to_owned(),
        };
        let (reason, detected) = classify(&[], "某站", false, Some(&failure), None);
        assert_eq!(reason, "resolve-site-l3-failed");
        assert!(
            detected.iter().any(|item| item.contains("robots.txt")),
            "证据要保留 robots 原因：{detected:?}"
        );
    }

    #[test]
    fn classify_offline_says_offline() {
        let (reason, detected) = classify(&[candidate(55)], "example", true, None, None);
        assert_eq!(reason, "resolve-site-offline");
        assert!(detected.iter().any(|item| item.contains("offline")));
    }

    #[test]
    fn classify_no_match_vs_low_confidence() {
        let (reason, _) = classify(&[], "某站", false, None, None);
        assert_eq!(reason, "resolve-site-no-match");
        let (reason, detected) = classify(&[candidate(55)], "example", false, None, None);
        assert_eq!(reason, "resolve-site-low-confidence");
        assert!(detected.iter().any(|item| item.contains("55")));
    }

    #[test]
    fn classify_serp_failure_is_named_and_keeps_evidence() {
        let serp = SerpFailure::Unavailable {
            code: "BLOCKED".to_owned(),
            message: "robots.txt disallows https://example.com/serp".to_owned(),
        };
        let (reason, detected) = classify(&[], "某站", false, None, Some(&serp));
        assert_eq!(reason, "resolve-site-serp-failed");
        assert!(
            detected.iter().any(|item| item.contains("SERP")),
            "证据要说清是 SERP 失败：{detected:?}"
        );
    }

    #[test]
    fn classify_reports_both_l3_and_serp_failures() {
        let l3 = L3Failure::NoMatch;
        let serp = SerpFailure::Empty;
        let (reason, detected) = classify(&[], "某站", false, Some(&l3), Some(&serp));
        assert_eq!(reason, "resolve-site-l3-failed", "L3 失败优先");
        assert!(
            detected.iter().any(|item| item.contains("SERP")),
            "同时跑了 L5 时证据不能丢：{detected:?}"
        );
    }

    /// L5 默认关：不写 allow_serp_fallback 时连层都不出现，data 里也标 false。
    #[tokio::test]
    async fn serp_fallback_is_off_by_default() {
        let value = tool_resolve_site(&offline_ctx("serp-off"), &json!({"site": "example"})).await;
        assert_eq!(value["data"]["serp_fallback"], false);
        assert_eq!(value["data"]["layers"], json!(["l1"]));
    }

    /// offline 下即使显式开启 L5 也不会联网，且要如实说明。
    #[tokio::test]
    async fn serp_fallback_never_networks_in_offline_mode() {
        let value = tool_resolve_site(
            &offline_ctx("serp-offline"),
            &json!({"site": "example", "allow_serp_fallback": true}),
        )
        .await;
        assert_eq!(value["data"]["serp_fallback"], true);
        assert_eq!(
            value["data"]["layers"],
            json!(["l1"]),
            "offline 不得出现 serp-fallback 层"
        );
        let detected = value["meta"]["actionable"]["detected"]
            .as_array()
            .expect("detected");
        assert!(
            detected
                .iter()
                .any(|item| item.as_str().is_some_and(|text| text.contains("offline"))),
            "要如实说 offline：{detected:?}"
        );
    }
}
