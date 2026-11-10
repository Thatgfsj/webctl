//! `web_search` 工具（0.2.2 M1 + M3，议题 #199 / #200 / #202）：按查询词全网搜索。
//!
//! 语义（与 `docs/ROADMAP-0.2.2.md` 一致）：
//! - 默认 Bing（HTML SERP）；`engines` 可选多引擎（bing / baidu / 360 已接线）；
//! - 多引擎结果按归一化 URL 去重，再用 **RRF** 融合排序（rank 不可跨引擎比较，不宣称相关性分数）；
//! - 结果按**不可信页面内容**处理；**不自动抓正文**（正文用 fetch/extract）；
//! - 每次请求前 50–500ms 随机抖动（在 `web_search::engine::fetch_html` 内完成）；
//! - 合规口径：默认不因 robots 拒绝而放弃（用户决策：搜索优先），但 `notes` 明示；
//!   `WEBCTL_SEARCH_STRICT_ROBOTS=1` 时交给 `PoliteFetcher` 拦截（禁则 0 请求 + BLOCKED）；
//! - 失败语义：单引擎失败沿用该引擎原因；多引擎**部分失败** → `DEGRADED + search-partial`，
//!   **全部失败** → `DEGRADED + search-all-failed`（都带 actionable）。

use std::time::Instant;

use serde_json::Value;
use webctl_core::{Envelope, ErrorCode, Trust};
use webctl_web::politeness::{PoliteFetcher, PolitenessConfig};
use webctl_web::web_search::aggregate::{self, MergeOptions, Merged};
use webctl_web::web_search::{
    DEFAULT_MAX_RESULTS, DEFAULT_SNIPPET_MAX_CHARS, EngineSerp, EngineStatus, Jitter,
    MAX_RESULTS_LIMIT, SearchEngine, SearchReport, baidu, bing, engine, is_wired, so360,
    unavailable_note,
};

use crate::context::McpContext;
use crate::dispatch::{parse, to_value};
use crate::params::WebSearchParams;

/// 严格 robots 模式的开关（默认关；用户决策：搜索优先、合规其次）。
pub const STRICT_ROBOTS_ENV: &str = "WEBCTL_SEARCH_STRICT_ROBOTS";
/// 单次最多翻到第几页（防止无界翻页；各引擎按自身参数映射）。
pub const MAX_PAGE: usize = 5;

/// SERP 请求用的浏览器化 UA（简单反爬；不伪装成具体账号）。
const SERP_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36";

/// SERP 请求附加头（简单反爬：语言、导航意图、来源）。
fn serp_headers() -> Vec<(String, String)> {
    vec![
        (
            "accept".to_owned(),
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".to_owned(),
        ),
        (
            "accept-language".to_owned(),
            "zh-CN,zh;q=0.9,en;q=0.8".to_owned(),
        ),
        ("sec-fetch-dest".to_owned(), "document".to_owned()),
        ("sec-fetch-mode".to_owned(), "navigate".to_owned()),
        ("sec-fetch-site".to_owned(), "same-origin".to_owned()),
        ("sec-fetch-user".to_owned(), "?1".to_owned()),
        ("upgrade-insecure-requests".to_owned(), "1".to_owned()),
        ("referer".to_owned(), "https://www.bing.com/".to_owned()),
    ]
}

/// 严格模式是否开启。
fn strict_robots() -> bool {
    std::env::var(STRICT_ROBOTS_ENV)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// 解析 `engines`：默认 `[bing]`；未知引擎 `PARSE`；未接线引擎不拒绝，交给状态如实上报。
fn resolve_engines(raw: Option<&Vec<String>>) -> Result<Vec<SearchEngine>, (ErrorCode, String)> {
    let names: Vec<String> = match raw {
        Some(names) if !names.is_empty() => names.clone(),
        _ => vec![SearchEngine::DEFAULT.as_str().to_owned()],
    };
    let mut engines = Vec::new();
    for name in names {
        let Some(engine) = SearchEngine::parse(&name) else {
            return Err((
                ErrorCode::Parse,
                format!(
                    "未知引擎 {name:?}；可选：bing（默认）/ baidu / 360 / mojeek / marginalia / yandex"
                ),
            ));
        };
        if !engines.contains(&engine) {
            engines.push(engine);
        }
    }
    Ok(engines)
}

/// 引擎的 SERP 基址：调试覆盖优先，否则用各引擎默认。
fn base_for(ctx: &McpContext, engine: SearchEngine) -> String {
    if let Some(base) = ctx.search_base.as_ref() {
        return base.clone();
    }
    match engine {
        SearchEngine::Baidu => baidu::DEFAULT_BASE.to_owned(),
        SearchEngine::Sogou => so360::DEFAULT_BASE.to_owned(),
        _ => bing::base_url(),
    }
}

/// 合规与口径说明（每次结果都带，不依赖调用方记忆）。
fn compliance_notes(strict: bool, page: usize, engines: &[SearchEngine]) -> Vec<String> {
    let names: Vec<&str> = engines.iter().map(|e| e.as_str()).collect();
    let mut notes = vec![
        format!("engines: {}", names.join(", ")),
        "compliance: HTML SERP——Bing/Baidu/360 的 robots.txt 均禁止搜索路径；默认按用户决策走搜索（搜索优先、合规其次）。"
            .to_owned(),
        "trust: 结果是引擎给出的不可信页面内容；摘要（若有）是生成内容，必须打开原始链接核对。".to_owned(),
        "next: 正文请用 fetch / extract 打开结果里的原始链接；本工具不自动抓页面。".to_owned(),
    ];
    if strict {
        notes.push(format!(
            "strict_robots: {STRICT_ROBOTS_ENV}=1——robots 禁止时不会发请求（BLOCKED + 0 请求）。"
        ));
    }
    if page > 1 {
        notes.push(format!("page: 第 {page} 页（上限 {MAX_PAGE}）。"));
    }
    notes
}

/// 降级来源 → 逐引擎状态名。
fn status_of(degraded_from: Option<&str>) -> &'static str {
    match degraded_from {
        None => "hit",
        Some("search-empty") => "empty",
        Some("search-blocked") => "blocked",
        Some("search-timeout") => "timeout",
        Some("search-http-error") => "http-error",
        Some("search-not-html") => "not-html",
        Some(_) => "error",
    }
}

fn status_for(engine: SearchEngine, serp: &Envelope<EngineSerp>) -> EngineStatus {
    // 硬失败（ok=false，例如 NETWORK）**不能**报 hit：状态给 failed，note 带错误原因。
    if !serp.ok {
        return EngineStatus {
            engine: engine.as_str().to_owned(),
            status: "failed".to_owned(),
            count: 0,
            elapsed_ms: serp.meta.elapsed_ms,
            note: Some(serp.error.clone()),
        };
    }
    let degraded = serp.meta.degraded_from.as_deref();
    EngineStatus {
        engine: engine.as_str().to_owned(),
        status: status_of(degraded).to_owned(),
        count: serp
            .data
            .as_ref()
            .map(|serp| serp.results.len())
            .unwrap_or(0),
        elapsed_ms: serp.meta.elapsed_ms,
        note: degraded.map(str::to_owned),
    }
}

/// `web_search` 的派发实现。
pub(crate) async fn tool_web_search(ctx: &McpContext, args: &Value) -> Value {
    let params: WebSearchParams = match parse(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    let query = params.query.trim().to_owned();
    if query.is_empty() {
        return to_value(Envelope::<SearchReport>::fail(
            ErrorCode::Parse,
            "query 不能为空",
            0,
        ));
    }
    let max_results = params
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .clamp(1, MAX_RESULTS_LIMIT);
    let page = params.page.unwrap_or(1).clamp(1, MAX_PAGE);
    let engines = match resolve_engines(params.engines.as_ref()) {
        Ok(engines) => engines,
        Err((code, message)) => return to_value(Envelope::<SearchReport>::fail(code, message, 0)),
    };
    // 排除操作符：先规范化（补 '-'、去空、去重），再原样透传给每个引擎。
    let exclude = engine::normalize_exclude(params.exclude.as_deref().unwrap_or_default());
    // 紧凑性（0.2.3 M3）：默认折叠同域 + 截断到 300 字符；显式参数可还原。
    let expand_domains = params.expand_domains.unwrap_or(false);
    let snippet_max_chars = params
        .snippet_max_chars
        .unwrap_or(DEFAULT_SNIPPET_MAX_CHARS);

    let strict = strict_robots();
    let config = PolitenessConfig {
        respect_robots: strict,
        ..PolitenessConfig::default()
    };
    let mut options = ctx.fetch_options();
    options.user_agent = Some(SERP_USER_AGENT.to_owned());
    options.headers.extend(serp_headers());
    let fetcher = PoliteFetcher::new(config, options);
    let jitter = Jitter::default();

    let started = Instant::now();
    let mut statuses = Vec::new();
    let mut inputs: Vec<(SearchEngine, Vec<webctl_web::web_search::SearchHit>)> = Vec::new();
    let mut attempted = 0usize;
    let mut succeeded = 0usize;
    let mut filtered_ads = 0usize;
    let mut single_reason: Option<String> = None;
    let mut hard_failure: Option<(webctl_core::ErrorCode, String)> = None;
    let mut related = Vec::new();
    for engine in &engines {
        if !is_wired(*engine) {
            statuses.push(EngineStatus {
                engine: engine.as_str().to_owned(),
                status: "unavailable".to_owned(),
                count: 0,
                elapsed_ms: 0,
                note: Some(unavailable_note(*engine).to_owned()),
            });
            continue;
        }
        attempted += 1;
        let base = base_for(ctx, *engine);
        let serp = match engine {
            SearchEngine::Bing => engine::map_serp(
                bing::fetch_serp(
                    &base,
                    &query,
                    page,
                    max_results,
                    &exclude,
                    &jitter,
                    &fetcher,
                )
                .await,
                EngineSerp::from,
            ),
            SearchEngine::Baidu => {
                baidu::fetch_serp(
                    &base,
                    &query,
                    page,
                    max_results,
                    &exclude,
                    &jitter,
                    &fetcher,
                )
                .await
            }
            SearchEngine::Sogou => {
                so360::fetch_serp(
                    &base,
                    &query,
                    page,
                    max_results,
                    &exclude,
                    &jitter,
                    &fetcher,
                )
                .await
            }
            _ => unreachable!("is_wired 已过滤"),
        };
        statuses.push(status_for(*engine, &serp));
        if !serp.ok {
            // 硬失败：保留 code/error，供单引擎时按 M1 语义原样 fail。
            if hard_failure.is_none() {
                hard_failure = Some((serp.code, serp.error.clone()));
            }
            continue;
        }
        let degraded = serp.meta.degraded_from.clone();
        if let Some(data) = &serp.data {
            // 即使结果为空/降级，被剔除的高置信广告也要计数（不静默删除）。
            filtered_ads += data.filtered_ads;
        }
        if degraded.is_none()
            && let Some(data) = &serp.data
        {
            succeeded += 1;
            inputs.push((*engine, data.results.clone()));
            related.extend(data.related.iter().cloned());
            continue;
        }
        if single_reason.is_none() {
            single_reason = degraded;
        }
    }

    // 单引擎同样走 merge：保证单/多引擎行为一致（折叠、截断都由聚合层完成）。
    let Merged { results, .. } = aggregate::merge(
        &inputs,
        MergeOptions {
            max_results,
            expand_domains,
            snippet_max_chars,
        },
    );
    related.sort_by(|a, b| a.url.cmp(&b.url));
    related.dedup_by(|a, b| a.url == b.url);

    let report = SearchReport {
        query: query.clone(),
        results,
        filtered_ads,
        applied_operators: exclude.clone(),
        engines: statuses,
        llm_summary: None,
        related,
        notes: compliance_notes(strict, page, &engines),
    };
    let elapsed = started.elapsed().as_millis() as u64;
    // 单引擎硬失败（无降级原因）保持 M1 语义：原样 fail(code, error)，不伪装成降级。
    if engines.len() == 1
        && attempted == 1
        && succeeded == 0
        && single_reason.is_none()
        && let Some((code, message)) = hard_failure.clone()
    {
        return to_value(Envelope::<SearchReport>::fail(code, message, elapsed));
    }
    // 排除操作符生效校验：只看本次返回的结果，不发额外请求（best-effort）。
    let ignored = succeeded > 0
        && !exclude.is_empty()
        && engine::operators_ignored(&report.results, &exclude);
    let envelope = if attempted == 0 || succeeded == 0 {
        // 单引擎失败时保留它自己的原因（与 M1 行为一致）；多引擎全失败用 search-all-failed。
        let reason = if attempted <= 1 {
            single_reason.unwrap_or_else(|| "search-all-failed".to_owned())
        } else {
            "search-all-failed".to_owned()
        };
        Envelope::degraded(report, reason, ErrorCode::Degraded, elapsed)
    } else {
        // 部分引擎失败与操作符被忽略可以同时成立：degraded_from 用逗号组合（既有机制）。
        let mut reasons: Vec<&str> = Vec::new();
        if succeeded < engines.len() {
            reasons.push("search-partial");
        }
        if ignored {
            reasons.push("search-operator-ignored");
        }
        if reasons.is_empty() {
            Envelope::ok(report, Trust::UntrustedPageContent, elapsed)
        } else {
            Envelope::degraded(report, reasons.join(","), ErrorCode::Degraded, elapsed)
        }
    };
    to_value(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_engines_is_bing_only() {
        assert_eq!(
            resolve_engines(None).expect("默认引擎"),
            vec![SearchEngine::Bing]
        );
    }

    #[test]
    fn multiple_engines_are_kept_in_order_and_deduped() {
        let engines = resolve_engines(Some(&vec![
            "bing".to_owned(),
            "baidu".to_owned(),
            "bing".to_owned(),
        ]))
        .expect("多引擎");
        assert_eq!(engines, vec![SearchEngine::Bing, SearchEngine::Baidu]);
    }

    #[test]
    fn unknown_engine_is_parse_error() {
        let error = resolve_engines(Some(&vec!["google".to_owned()])).expect_err("未知引擎应报错");
        assert_eq!(error.0, ErrorCode::Parse);
    }

    #[test]
    fn unwired_engine_is_not_rejected() {
        let engines = resolve_engines(Some(&vec!["mojeek".to_owned()])).expect("未接线不拒绝");
        assert_eq!(engines, vec![SearchEngine::Mojeek]);
        assert!(!is_wired(SearchEngine::Mojeek));
    }

    #[test]
    fn status_maps_degraded_reasons() {
        assert_eq!(status_of(None), "hit");
        assert_eq!(status_of(Some("search-empty")), "empty");
        assert_eq!(status_of(Some("search-blocked")), "blocked");
        assert_eq!(status_of(Some("search-timeout")), "timeout");
        assert_eq!(status_of(Some("whatever")), "error");
    }

    #[test]
    fn compliance_notes_list_engines_and_disclose_robots() {
        let notes = compliance_notes(false, 1, &[SearchEngine::Bing, SearchEngine::Baidu]);
        let joined = notes.join(" ");
        assert!(joined.contains("bing, baidu"));
        assert!(joined.contains("robots"));
        assert!(joined.contains("不可信"));
    }

    #[test]
    fn strict_mode_adds_note() {
        let notes = compliance_notes(true, 1, &[SearchEngine::Bing]);
        assert!(notes.iter().any(|note| note.starts_with("strict_robots")));
    }
}
