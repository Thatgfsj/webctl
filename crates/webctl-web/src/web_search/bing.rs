//! Bing SERP（HTML）适配：URL 构造、结果解析、单次取回（0.2.2 M1，议题 #200）。
//!
//! 实测（2026-11-01）：`https://www.bing.com/search` 会重定向到 `cn.bing.com`，
//! 返回的 HTML 里 `li.b_algo` 是自然结果块，`h2 a` 是标题链接，`p` 是摘要，`cite` 是展示域名。
//! 解析器只认这些结构；认不出就返回空 + 让上层如实降级（不猜、不编）。

use std::time::Duration;

use scraper::{Html, Selector};
use url::Url;
use webctl_core::{Envelope, ErrorCode, Trust};

use crate::politeness::PoliteFetcher;
use crate::web_search::{EngineStatus, Jitter, RelatedTopic, SearchHit, SourceLink, hash_salt};

/// 默认 SERP 基址。
pub const DEFAULT_BASE: &str = "https://www.bing.com/search";
/// 调试/测试用基址覆盖（离线 lane 用 127.0.0.1 fixture）。
pub const BASE_ENV: &str = "WEBCTL_SEARCH_BASE";

/// 一次 Bing SERP 的解析结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BingSerp {
    /// 自然结果。
    pub results: Vec<SearchHit>,
    /// 相关主题 / 相关搜索。
    pub related: Vec<RelatedTopic>,
}

/// 解析结果 + 拦截特征（内部用）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Parsed {
    serp: BingSerp,
    looks_blocked: bool,
}

/// 构造搜索 URL。
///
/// - `first` 从 1 开始（Bing 的分页参数）；
/// - `count` 作为提示传 `max_results`，最终以解析结果为准。
pub fn search_url(base: &str, query: &str, page: usize, max_results: usize) -> String {
    let page = page.max(1);
    let first = 1 + (page - 1) * 10;
    let encoded = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
    let join = if base.contains('?') { '&' } else { '?' };
    format!("{base}{join}q={encoded}&first={first}&count={max_results}")
}

/// 当前 SERP 基址（环境变量可覆盖，供离线测试）。
pub fn base_url() -> String {
    std::env::var(BASE_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE.to_owned())
}

/// 解析 Bing SERP 的 HTML。
pub fn parse_serp(html: &str, max_results: usize) -> BingSerp {
    parse_internal(html, max_results).serp
}

fn parse_internal(html: &str, max_results: usize) -> Parsed {
    let lowered = html.to_ascii_lowercase();
    let looks_blocked = [
        "captcha",
        "unusual traffic",
        "verify you are human",
        "challenge-form",
    ]
    .iter()
    .any(|marker| lowered.contains(marker));
    let document = Html::parse_document(html);
    let block_selector = Selector::parse("li.b_algo").expect("静态选择器");
    let title_selector = Selector::parse("h2 a").expect("静态选择器");
    let link_selector = Selector::parse("a[href]").expect("静态选择器");
    let paragraph_selector = Selector::parse("p").expect("静态选择器");
    let cite_selector = Selector::parse("cite").expect("静态选择器");

    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for block in document.select(&block_selector) {
        if results.len() >= max_results {
            break;
        }
        let anchor = block
            .select(&title_selector)
            .next()
            .or_else(|| block.select(&link_selector).next());
        let Some(anchor) = anchor else { continue };
        let href = anchor.value().attr("href").unwrap_or_default();
        let Some(url) = normalize_result_url(href) else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        }
        let title = anchor.text().collect::<String>().trim().to_owned();
        let snippet = block
            .select(&paragraph_selector)
            .map(|p| p.text().collect::<String>().trim().to_owned())
            .find(|text| !text.is_empty())
            .unwrap_or_default();
        let display_url = block
            .select(&cite_selector)
            .next()
            .map(|cite| cite.text().collect::<String>().trim().to_owned())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| url.clone());
        if title.is_empty() {
            continue;
        }
        results.push(SearchHit {
            rank: results.len() as u32 + 1,
            engine: "bing".to_owned(),
            title,
            url: url.clone(),
            display_url,
            snippet,
            engines: vec!["bing".to_owned()],
        });
    }

    let mut related = Vec::new();
    let related_selector =
        Selector::parse("#b_context a[href], #b_rs a[href]").expect("静态选择器");
    for anchor in document.select(&related_selector) {
        let Some(url) = anchor.value().attr("href").and_then(normalize_result_url) else {
            continue;
        };
        let title = anchor.text().collect::<String>().trim().to_owned();
        if title.is_empty() {
            continue;
        }
        related.push(RelatedTopic { title, url });
        if related.len() >= 8 {
            break;
        }
    }

    Parsed {
        serp: BingSerp { results, related },
        looks_blocked,
    }
}

/// 结果链接归一化：只保留 http(s)；Bing 的 `/ck/a?...u=a1<base64>` 跳转链解开；
/// 引擎自身域名与非 http(s) 一律丢弃（避免把跳转链当结果）。
pub fn normalize_result_url(href: &str) -> Option<String> {
    let url = Url::parse(href).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if is_site(&host, "bing.com") && url.path().starts_with("/ck/a") {
        let encoded = url
            .query_pairs()
            .find(|(key, _)| key == "u")
            .map(|(_, value)| value.into_owned())?;
        // Bing 跳转链固定是 u=a1<base64url>；没有 a1 前缀的不猜（避免把普通查询参数当跳转链）。
        let payload = encoded.strip_prefix("a1")?;
        let decoded = base64_url_decode(payload)?;
        let target = Url::parse(&decoded).ok()?;
        if !matches!(target.scheme(), "http" | "https") {
            return None;
        }
        return Some(strip_fragment(&target));
    }
    if is_site(&host, "bing.com") || is_site(&host, "microsoft.com") {
        return None;
    }
    Some(strip_fragment(&url))
}

/// host 是否属于某站点（精确匹配或 . 子域；不能用裸 ends_with，否则 evilbing.com 会被误判）。
fn is_site(host: &str, site: &str) -> bool {
    host == site || host.ends_with(&format!(".{site}"))
}

fn strip_fragment(url: &Url) -> String {
    let mut trimmed = url.clone();
    trimmed.set_fragment(None);
    trimmed.to_string()
}

/// 最小 base64url 解码（Bing 的跳转链用；失败返回 None，不猜）。
fn base64_url_decode(input: &str) -> Option<String> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut bits: u32 = 0;
    let mut count = 0u32;
    let mut out = Vec::new();
    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let value = TABLE.iter().position(|c| *c == byte)? as u32;
        bits = (bits << 6) | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    String::from_utf8(out).ok()
}

/// 取回并解析一次 Bing SERP（带随机抖动）。
///
/// 失败语义：被拦 → `DEGRADED + search-blocked`；超时 → `DEGRADED + search-timeout`；
/// HTTP 错误 / 空结果 → `DEGRADED + search-http-error` / `search-empty`。
pub async fn fetch_serp(
    base: &str,
    query: &str,
    page: usize,
    max_results: usize,
    jitter: &Jitter,
    fetcher: &PoliteFetcher,
) -> Envelope<BingSerp> {
    let url = search_url(base, query, page, max_results);
    let delay = jitter.delay_ms(hash_salt(&url));
    tokio::time::sleep(Duration::from_millis(delay)).await;
    let response = fetcher.fetch(&url).await;
    let elapsed = response.meta.elapsed_ms;
    if !response.ok {
        return match response.code {
            ErrorCode::Blocked => Envelope::degraded(
                BingSerp::default(),
                "search-blocked",
                ErrorCode::Degraded,
                elapsed,
            ),
            ErrorCode::Timeout => Envelope::degraded(
                BingSerp::default(),
                "search-timeout",
                ErrorCode::Degraded,
                elapsed,
            ),
            other => Envelope::fail(other, response.error, elapsed),
        };
    }
    let Some(polite) = response.data else {
        return Envelope::fail(ErrorCode::Network, "SERP 响应为空", elapsed);
    };
    let fetched = polite.fetched;
    if fetched.status >= 400 {
        return Envelope::degraded(
            BingSerp::default(),
            "search-http-error",
            ErrorCode::Degraded,
            elapsed,
        );
    }
    let Some(html) = fetched.content.text else {
        return Envelope::degraded(
            BingSerp::default(),
            "search-not-html",
            ErrorCode::Degraded,
            elapsed,
        );
    };
    let parsed = parse_internal(&html, max_results);
    if parsed.looks_blocked {
        return Envelope::degraded(
            BingSerp::default(),
            "search-blocked",
            ErrorCode::Degraded,
            elapsed,
        );
    }
    if parsed.serp.results.is_empty() {
        return Envelope::degraded(
            BingSerp {
                results: Vec::new(),
                related: parsed.serp.related,
            },
            "search-empty",
            ErrorCode::Degraded,
            elapsed,
        );
    }
    Envelope::ok(parsed.serp, Trust::UntrustedPageContent, elapsed)
}

/// 供调用方拼 `engines[]` 状态（M1 只有 Bing）。
pub fn engine_status(results: usize, elapsed_ms: u64, degraded_from: Option<&str>) -> EngineStatus {
    let (status, note) = match degraded_from {
        None => ("hit", None),
        Some(reason) => {
            let status = match reason {
                "search-empty" => "empty",
                "search-blocked" => "blocked",
                "search-timeout" => "timeout",
                _ => "unavailable",
            };
            (status, Some(reason.to_owned()))
        }
    };
    EngineStatus {
        engine: "bing".to_owned(),
        status: status.to_owned(),
        count: results,
        elapsed_ms,
        note,
    }
}

/// 摘要来源（M2 用；M1 不产生摘要，保留类型以便 `SearchReport` 稳定）。
pub fn source_links(_html: &str) -> Vec<SourceLink> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERP: &str = r#"<html><body><ol id="b_results">
      <li class="b_algo"><h2><a href="https://example.com/a">First result</a></h2>
        <div class="b_caption"><p>First snippet.</p></div><cite>example.com</cite></li>
      <li class="b_algo"><h2><a href="https://example.org/b">Second result</a></h2>
        <div class="b_caption"><p>Second snippet.</p></div><cite>example.org</cite></li>
    </ol><div id="b_context"><a href="https://example.net/related">Related topic</a></div></body></html>"#;

    #[test]
    fn parses_two_results_and_related() {
        let serp = parse_serp(SERP, 10);
        assert_eq!(serp.results.len(), 2);
        assert_eq!(serp.results[0].rank, 1);
        assert_eq!(serp.results[0].title, "First result");
        assert_eq!(serp.results[0].url, "https://example.com/a");
        assert_eq!(serp.results[0].snippet, "First snippet.");
        assert_eq!(serp.results[0].display_url, "example.com");
        assert_eq!(serp.related.len(), 1);
    }

    #[test]
    fn respects_max_results() {
        assert_eq!(parse_serp(SERP, 1).results.len(), 1);
    }

    #[test]
    fn empty_page_is_empty_not_error() {
        assert!(
            parse_serp("<html><body></body></html>", 10)
                .results
                .is_empty()
        );
    }

    #[test]
    fn detects_captcha_markers() {
        let parsed = parse_internal("<html><body>unusual traffic - captcha</body></html>", 10);
        assert!(parsed.looks_blocked);
    }

    #[test]
    fn search_url_encodes_query_and_paginates() {
        let url = search_url(DEFAULT_BASE, "rust 网页", 2, 10);
        assert!(url.contains("q=rust+%E7%BD%91%E9%A1%B5"), "{url}");
        assert!(url.contains("first=11"));
        assert!(url.contains("count=10"));
    }

    #[test]
    fn result_url_decoding_and_filtering() {
        assert_eq!(
            normalize_result_url("https://example.com/x#frag").as_deref(),
            Some("https://example.com/x")
        );
        assert_eq!(
            normalize_result_url("https://www.bing.com/search?q=x"),
            None
        );
        assert_eq!(normalize_result_url("javascript:alert(1)"), None);
    }

    #[test]
    fn site_suffix_check_is_not_naive_ends_with() {
        assert!(is_site("bing.com", "bing.com"));
        assert!(is_site("cn.bing.com", "bing.com"));
        assert!(!is_site("evilbing.com", "bing.com"));
        assert!(!is_site("bing.com.evil.test", "bing.com"));
    }

    #[test]
    fn ck_a_wrapper_is_unwrapped() {
        // u=a1 + base64url(https://example.com/target)
        let href = "https://www.bing.com/ck/a?!&&u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS90YXJnZXQ&ntb=1";
        assert_eq!(
            normalize_result_url(href).as_deref(),
            Some("https://example.com/target")
        );
    }

    #[test]
    fn ck_a_without_prefix_or_bad_payload_is_dropped() {
        // 没有 a1 前缀：即使载荷能解出合法 URL 也必须拒绝（不猜跳转链格式）
        let href = "https://www.bing.com/ck/a?u=aHR0cHM6Ly9leGFtcGxlLmNvbS94";
        assert_eq!(normalize_result_url(href), None);
        // 有前缀但解出非 URL → None
        let href = "https://www.bing.com/ck/a?u=a1bm90LWEtdXJs";
        assert_eq!(normalize_result_url(href), None);
        // 非法 base64url 字符 → None（不 panic）
        let href = "https://www.bing.com/ck/a?u=a1***";
        assert_eq!(normalize_result_url(href), None);
        // 解码后不是 http(s) → None
        let href = "https://www.bing.com/ck/a?u=a1amF2YXNjcmlwdDphbGVydCgxKQ";
        assert_eq!(normalize_result_url(href), None);
    }

    #[test]
    fn engine_status_maps_degrade_reasons() {
        assert_eq!(engine_status(3, 42, None).status, "hit");
        assert_eq!(engine_status(0, 42, Some("search-empty")).status, "empty");
        assert_eq!(
            engine_status(0, 42, Some("search-blocked")).status,
            "blocked"
        );
    }
}
