//! `web_search` 工具（0.2.2 M1，议题 #199 / #200）：按查询词全网搜索。
//!
//! 语义（与 `docs/ROADMAP-0.2.2.md` 一致）：
//! - 默认 Bing（HTML SERP）；结果按**不可信页面内容**处理；**不自动抓正文**（正文用 fetch/extract）；
//! - 每次请求前 50–500ms 随机抖动（在 `web_search::bing::fetch_serp` 内完成）；
//! - 合规口径：默认不因 robots 拒绝而放弃（用户决策：搜索优先），但 `notes` 明示；
//!   `WEBCTL_SEARCH_STRICT_ROBOTS=1` 时交给 `PoliteFetcher` 拦截（禁则 0 请求 + BLOCKED）；
//! - 失败语义：被拦 / 空结果 / 超时 → `DEGRADED` + `degraded_from` + `actionable`。

use std::time::Instant;

use serde_json::Value;
use webctl_core::{Envelope, ErrorCode, Trust};
use webctl_web::politeness::{PoliteFetcher, PolitenessConfig};
use webctl_web::web_search::{
    DEFAULT_MAX_RESULTS, EngineStatus, Jitter, MAX_RESULTS_LIMIT, SearchEngine, SearchReport, bing,
};

use crate::context::McpContext;
use crate::dispatch::{parse, to_value};
use crate::params::WebSearchParams;

/// 严格 robots 模式的开关（默认关；用户决策：搜索优先、合规其次）。
pub const STRICT_ROBOTS_ENV: &str = "WEBCTL_SEARCH_STRICT_ROBOTS";

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

/// 解析 `engines`：默认 `[bing]`；未知/未接线引擎如实报错，不静默替换。
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
                    "未知引擎 {name:?}；可选：bing（默认）/ mojeek / marginalia / baidu / 360 / yandex（0.2.2 M1 仅 bing 已接线）"
                ),
            ));
        };
        if engine != SearchEngine::Bing {
            return Err((
                ErrorCode::Unsupported,
                format!(
                    "引擎 {} 尚未接线（0.2.2 M3 多引擎）；当前只支持 bing",
                    engine.as_str()
                ),
            ));
        }
        if !engines.contains(&engine) {
            engines.push(engine);
        }
    }
    Ok(engines)
}

/// 合规与口径说明（每次结果都带，不依赖调用方记忆）。
fn compliance_notes(strict: bool, page: usize) -> Vec<String> {
    let mut notes = vec![
        "compliance: HTML SERP——Bing 的 robots.txt 禁止 /search；默认按用户决策走搜索（搜索优先、合规其次）。"
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
        notes.push(format!(
            "page: 第 {page} 页（first={}）；不同引擎的翻页能力不同。",
            1 + (page - 1) * 10
        ));
    }
    notes
}

fn engine_statuses(
    results: usize,
    elapsed_ms: u64,
    degraded_from: Option<&str>,
) -> Vec<EngineStatus> {
    vec![bing::engine_status(results, elapsed_ms, degraded_from)]
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
    let page = params.page.unwrap_or(1).max(1);
    if let Err((code, message)) = resolve_engines(params.engines.as_ref()) {
        return to_value(Envelope::<SearchReport>::fail(code, message, 0));
    }

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
    let base = ctx
        .search_base
        .clone()
        .unwrap_or_else(webctl_web::web_search::bing::base_url);
    let serp = bing::fetch_serp(&base, &query, page, max_results, &jitter, &fetcher).await;
    let elapsed = started.elapsed().as_millis() as u64;
    let degraded_from = serp.meta.degraded_from.clone();

    let mut report = match serp.data {
        Some(serp) => {
            let count = serp.results.len();
            SearchReport {
                query: query.clone(),
                results: serp.results,
                engines: engine_statuses(count, elapsed, degraded_from.as_deref()),
                llm_summary: None,
                related: serp.related,
                notes: compliance_notes(strict, page),
            }
        }
        None => {
            let mut report = SearchReport::empty(&query);
            report.engines = engine_statuses(0, elapsed, degraded_from.as_deref());
            report.notes = compliance_notes(strict, page);
            report
        }
    };

    let envelope = if let Some(reason) = degraded_from {
        Envelope::degraded(report, reason, ErrorCode::Degraded, elapsed)
    } else if !serp.ok {
        Envelope::fail(serp.code, serp.error, elapsed)
    } else {
        // `serp.data` 已在上面的 match 里消费；这里只可能是 ok 且有数据的分支。
        report.engines = engine_statuses(report.results.len(), elapsed, None);
        Envelope::ok(report, Trust::UntrustedPageContent, elapsed)
    };
    to_value(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_engines_is_bing_only() {
        let engines = resolve_engines(None).expect("默认引擎");
        assert_eq!(engines, vec![SearchEngine::Bing]);
    }

    #[test]
    fn unknown_engine_is_parse_error() {
        let error = resolve_engines(Some(&vec!["google".to_owned()])).expect_err("未知引擎应报错");
        assert_eq!(error.0, ErrorCode::Parse);
    }

    #[test]
    fn not_wired_engine_is_unsupported() {
        let error = resolve_engines(Some(&vec!["mojeek".to_owned()])).expect_err("未接线应报错");
        assert_eq!(error.0, ErrorCode::Unsupported);
    }

    #[test]
    fn compliance_notes_always_disclose_serp_fact() {
        let notes = compliance_notes(false, 1);
        let joined = notes.join(" ");
        assert!(joined.contains("robots"));
        assert!(joined.contains("不可信"));
    }

    #[test]
    fn strict_mode_adds_note() {
        let notes = compliance_notes(true, 1);
        assert!(notes.iter().any(|note| note.starts_with("strict_robots")));
    }
}
