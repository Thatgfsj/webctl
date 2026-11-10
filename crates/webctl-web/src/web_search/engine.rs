//! 引擎适配公共层（0.2.2 M3，议题 #202）：统一结果类型、取回助手、失败映射。
//!
//! 每个引擎模块（`bing` / `baidu` / `so360`）只负责：URL 构造、HTML 解析、拦截特征；
//! 取回（抖动 + 礼貌层）与失败语义的统一映射放在这里，避免三份复制。

use std::collections::HashSet;
use std::time::Duration;

use url::Url;
use webctl_core::{Envelope, ErrorCode, Trust};

use crate::politeness::PoliteFetcher;

use super::bing::BingSerp;
use super::{Jitter, RelatedTopic, SearchEngine, SearchHit, hash_salt};

/// 单个引擎的一次搜索结果（多引擎聚合前的单位）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineSerp {
    /// 自然结果（已剔除高置信广告）。
    pub results: Vec<SearchHit>,
    /// 相关主题 / 相关搜索。
    pub related: Vec<RelatedTopic>,
    /// 被高置信广告规则剔除的条数（供上层汇总到 `SearchReport::filtered_ads`）。
    pub filtered_ads: usize,
}

impl From<BingSerp> for EngineSerp {
    fn from(serp: BingSerp) -> Self {
        Self {
            results: serp.results,
            related: serp.related,
            filtered_ads: serp.filtered_ads,
        }
    }
}

/// 已接线的引擎（有解析器 + 离线 fixture）。
pub fn is_wired(engine: SearchEngine) -> bool {
    matches!(
        engine,
        SearchEngine::Bing | SearchEngine::Baidu | SearchEngine::Sogou
    )
}

/// 未接线引擎的如实说明（不做请求，直接回报原因）。
pub fn unavailable_note(engine: SearchEngine) -> &'static str {
    match engine {
        SearchEngine::Mojeek => "实测返回验证码/拦截页，未接线（不发起请求）",
        SearchEngine::Marginalia => "新版 UI 是 JS 外壳，HTML 无结果块，未接线（不发起请求）",
        SearchEngine::Yandex => "尚未接线（需要专门的跳转链与反爬处理）",
        _ => "未接线",
    }
}

/// 规范化排除操作符：去首尾空白、丢弃空串、补齐 '-' 前缀、去重（保序）。
///
/// 用户可能传 `tutorial` 或 `-tutorial`，统一按后者透传与回显；
/// `site:csdn.net` 与 `-site:csdn.net` 同理。
pub fn normalize_exclude(items: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for item in items {
        let term = item.trim();
        if term.is_empty() || term == "-" {
            continue;
        }
        let operator = if term.starts_with('-') {
            term.to_owned()
        } else {
            format!("-{term}")
        };
        if seen.insert(operator.clone()) {
            normalized.push(operator);
        }
    }
    normalized
}

/// 把排除操作符拼到查询串后（`query + " " + operator`）；空列表不改动查询串。
pub fn append_operators(query: &str, exclude: &[String]) -> String {
    let mut combined = query.to_owned();
    for operator in normalize_exclude(exclude) {
        combined.push(' ');
        combined.push_str(&operator);
    }
    combined
}

/// 生效校验（best-effort，不额外发请求）：结果里是否仍出现被排除的 term / site。
///
/// 规则：
/// - `-site:host`：任一结果 URL 的 host 等于 host 或是其子域 → 认为引擎忽略了该操作符；
/// - 其它 `-term`：任一结果 title/snippet（小写）包含 term（小写）→ 认为忽略；
/// - 空 operators → false。
///
/// 局限：只能看"返回了什么"；引擎若在服务端排除干净就无法证明，反之自然命中也可能误判。
pub fn operators_ignored(results: &[SearchHit], operators: &[String]) -> bool {
    let normalized = normalize_exclude(operators);
    if normalized.is_empty() {
        return false;
    }
    normalized.iter().any(|operator| {
        let term = operator.strip_prefix('-').unwrap_or(operator).trim();
        if term.is_empty() {
            return false;
        }
        match term.strip_prefix("site:") {
            Some(site) => {
                let site = site.trim().trim_end_matches('/').to_ascii_lowercase();
                !site.is_empty() && results.iter().any(|hit| host_matches(&hit.url, &site))
            }
            None => {
                let needle = term.to_lowercase();
                results.iter().any(|hit| {
                    hit.title.to_lowercase().contains(&needle)
                        || hit.snippet.to_lowercase().contains(&needle)
                })
            }
        }
    })
}

/// URL host 是否等于 site 或其子域（不能裸 ends_with，避免 csdn.net.evil.test 误判）。
fn host_matches(url: &str, site: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    host == site || host.ends_with(&format!(".{site}"))
}

/// 取回到的 HTML 与实测耗时。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedHtml {
    /// 响应正文（已按 Content-Type 解码）。
    pub html: String,
    /// 取回耗时（毫秒，不含抖动）。
    pub elapsed_ms: u64,
}

/// 发一次带抖动的取回；失败原样返回（由 [`finish`] 统一映射成败语义）。
pub async fn fetch_html(
    url: &str,
    jitter: &Jitter,
    fetcher: &PoliteFetcher,
) -> Envelope<FetchedHtml> {
    let delay = jitter.delay_ms(hash_salt(url));
    tokio::time::sleep(Duration::from_millis(delay)).await;
    let response = fetcher.fetch(url).await;
    let elapsed = response.meta.elapsed_ms;
    if !response.ok {
        return Envelope::fail(response.code, response.error, elapsed);
    }
    let Some(polite) = response.data else {
        return Envelope::fail(ErrorCode::Network, "搜索响应为空", elapsed);
    };
    let fetched = polite.fetched;
    if fetched.status >= 400 {
        return Envelope::fail(
            ErrorCode::Network,
            format!("搜索页返回 HTTP {}", fetched.status),
            elapsed,
        );
    }
    let Some(html) = fetched.content.text else {
        return Envelope::fail(ErrorCode::Network, "搜索页不是文本内容", elapsed);
    };
    Envelope::ok(
        FetchedHtml {
            html,
            elapsed_ms: elapsed,
        },
        Trust::UntrustedPageContent,
        elapsed,
    )
}

/// 把一次取回 + 解析收口成统一语义：
///
/// - 被拦 / 超时 / HTTP 错误 / 空结果 → `DEGRADED`（带 `search-*` 来源，可行动信息由 core 自动补）；
/// - 有结果 → `OK`（解析出的条目）；
/// - 其余失败 → 原样 `fail`（不伪装成降级）。
pub fn finish<F>(
    fetched: Envelope<FetchedHtml>,
    looks_blocked: impl Fn(&str) -> bool,
    parse: F,
    max_results: usize,
) -> Envelope<EngineSerp>
where
    F: FnOnce(&str, usize) -> EngineSerp,
{
    let elapsed = fetched.meta.elapsed_ms;
    if !fetched.ok {
        return match fetched.code {
            ErrorCode::Blocked => Envelope::degraded(
                EngineSerp::default(),
                "search-blocked",
                ErrorCode::Degraded,
                elapsed,
            ),
            ErrorCode::Timeout => Envelope::degraded(
                EngineSerp::default(),
                "search-timeout",
                ErrorCode::Degraded,
                elapsed,
            ),
            ErrorCode::Network => {
                // 区分「HTTP 状态错误」与「连接失败」：前者是站点应答，后者是传输层。
                let reason = if fetched.error.contains("HTTP") {
                    "search-http-error"
                } else {
                    "search-network-error"
                };
                Envelope::degraded(EngineSerp::default(), reason, ErrorCode::Degraded, elapsed)
            }
            other => Envelope::fail(other, fetched.error, elapsed),
        };
    }
    let Some(page) = fetched.data else {
        return Envelope::fail(ErrorCode::Network, "搜索响应为空", elapsed);
    };
    if looks_blocked(&page.html) {
        return Envelope::degraded(
            EngineSerp::default(),
            "search-blocked",
            ErrorCode::Degraded,
            elapsed,
        );
    }
    let serp = parse(&page.html, max_results);
    if serp.results.is_empty() {
        return Envelope::degraded(serp, "search-empty", ErrorCode::Degraded, elapsed);
    }
    Envelope::ok(serp, Trust::UntrustedPageContent, elapsed)
}

/// 把信封的 data 映射成另一种类型（保留 ok/code/meta）。
pub fn map_serp<T>(env: Envelope<T>, f: impl FnOnce(T) -> EngineSerp) -> Envelope<EngineSerp> {
    Envelope {
        ok: env.ok,
        data: env.data.map(f),
        error: env.error,
        code: env.code,
        meta: env.meta,
    }
}

/// 是否命中任一特征（大小写不敏感）。
pub fn contains_any(html: &str, markers: &[&str]) -> bool {
    let lowered = html.to_ascii_lowercase();
    markers
        .iter()
        .any(|marker| lowered.contains(&marker.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wired_engines() {
        assert!(is_wired(SearchEngine::Bing));
        assert!(is_wired(SearchEngine::Baidu));
        assert!(is_wired(SearchEngine::Sogou));
        assert!(!is_wired(SearchEngine::Mojeek));
        assert!(!is_wired(SearchEngine::Marginalia));
        assert!(!is_wired(SearchEngine::Yandex));
    }

    #[test]
    fn unavailable_notes_are_honest() {
        assert!(unavailable_note(SearchEngine::Mojeek).contains("验证码"));
        assert!(unavailable_note(SearchEngine::Marginalia).contains("JS"));
        assert!(!unavailable_note(SearchEngine::Yandex).is_empty());
    }

    #[test]
    fn contains_any_is_case_insensitive() {
        assert!(contains_any(
            "<form class=\"Challenge-Form\">",
            &["challenge-form"]
        ));
        assert!(!contains_any("ok", &["captcha"]));
    }

    #[test]
    fn bing_serp_converts() {
        let serp = BingSerp {
            results: vec![SearchHit {
                rank: 1,
                engine: "bing".to_owned(),
                title: "t".to_owned(),
                url: "https://example.com/".to_owned(),
                display_url: "example.com".to_owned(),
                snippet: "s".to_owned(),
                engines: vec!["bing".to_owned()],
                is_ad: false,
                same_domain_count: None,
                snippet_truncated: false,
            }],
            related: Vec::new(),
            filtered_ads: 0,
        };
        let converted: EngineSerp = serp.into();
        assert_eq!(converted.results.len(), 1);
    }

    fn hit(url: &str, title: &str, snippet: &str) -> SearchHit {
        SearchHit {
            rank: 1,
            engine: "bing".to_owned(),
            title: title.to_owned(),
            url: url.to_owned(),
            display_url: url.to_owned(),
            snippet: snippet.to_owned(),
            engines: vec!["bing".to_owned()],
            is_ad: false,
            same_domain_count: None,
            snippet_truncated: false,
        }
    }

    #[test]
    fn normalize_exclude_prefixes_drops_blanks_and_dedupes() {
        let items = vec![
            " tutorial ".to_owned(),
            String::new(),
            "   ".to_owned(),
            "-site:csdn.net".to_owned(),
            "tutorial".to_owned(),
            "site:csdn.net".to_owned(),
        ];
        assert_eq!(
            normalize_exclude(&items),
            vec!["-tutorial".to_owned(), "-site:csdn.net".to_owned()]
        );
        assert!(normalize_exclude(&[]).is_empty());
    }

    #[test]
    fn operators_ignored_detects_sites_terms_and_empty() {
        let results = vec![hit("https://blog.csdn.net/x", "Rust 教程", "CSDN 博客")];
        assert!(operators_ignored(&results, &["-site:csdn.net".to_owned()]));
        assert!(operators_ignored(
            &results,
            &["tutorial".to_owned(), "-rust".to_owned()]
        ));
        assert!(!operators_ignored(&results, &[]));
        assert!(!operators_ignored(
            &results,
            &["-site:csdn.net.evil.test".to_owned()]
        ));
        assert!(!operators_ignored(&results, &["-python".to_owned()]));
    }
}
