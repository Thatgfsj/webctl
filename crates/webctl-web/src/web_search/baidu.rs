//! 百度 SERP（HTML）适配（0.2.2 M3，议题 #202）。
//!
//! 实测（2026-11-01）：`https://www.baidu.com/s?wd=…` 的 HTML 里自然结果是
//! `div.result.c-container`；块上的 **`mu` 属性就是真实目标 URL**，`/link?url=` 是跳转链、一律丢弃；
//! 标题在 `h3`，摘要在 `.c-abstract`。

use scraper::{Html, Selector};
use url::Url;
use webctl_core::Envelope;

use crate::politeness::PoliteFetcher;

use super::ads;
use super::engine::{EngineSerp, append_operators, contains_any, fetch_html, finish};
use super::{Jitter, SearchEngine, SearchHit};

/// 默认基址。
pub const DEFAULT_BASE: &str = "https://www.baidu.com/s";
/// 拦截特征（命中即 `search-blocked`）。
pub const BLOCKED_MARKERS: &[&str] = &["wappass", "百度安全验证", "验证码", "captcha"];

/// 构造搜索 URL。
///
/// `exclude` 规范化后拼到查询串末尾；百度对 `-term` 的支持不稳定，
/// 实际是否生效由上层 `operators_ignored` 校验（不在此处假装成功）。
pub fn search_url(
    base: &str,
    query: &str,
    page: usize,
    max_results: usize,
    exclude: &[String],
) -> String {
    let page = page.max(1);
    let pn = (page - 1) * 10;
    let query = append_operators(query, exclude);
    let encoded = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
    let join = if base.contains('?') { '&' } else { '?' };
    format!("{base}{join}wd={encoded}&pn={pn}&rn={max_results}")
}

/// 是否像拦截/验证码页。
pub fn looks_blocked(html: &str) -> bool {
    contains_any(html, BLOCKED_MARKERS)
}

/// 解析百度 SERP。
pub fn parse_serp(html: &str, max_results: usize) -> EngineSerp {
    let document = Html::parse_document(html);
    let block_selector = Selector::parse("div.result").expect("静态选择器");
    let title_selector = Selector::parse("h3").expect("静态选择器");
    let link_selector = Selector::parse("h3 a[href]").expect("静态选择器");
    let abstract_selector = Selector::parse(".c-abstract").expect("静态选择器");

    let mut results = Vec::new();
    let mut filtered_ads = 0usize;
    let mut seen = std::collections::HashSet::new();
    for block in document.select(&block_selector) {
        if results.len() >= max_results {
            break;
        }
        let raw_href = block
            .select(&link_selector)
            .next()
            .and_then(|a| a.value().attr("href"))
            .unwrap_or_default();
        let url = block
            .value()
            .attr("mu")
            .and_then(normalize_result_url)
            .or_else(|| normalize_result_url(raw_href));
        let Some(url) = url else { continue };
        let title = block
            .select(&title_selector)
            .next()
            .map(|h3| h3.text().collect::<String>().trim().to_owned())
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let snippet = block
            .select(&abstract_selector)
            .next()
            .map(|p| p.text().collect::<String>().trim().to_owned())
            .unwrap_or_default();
        let display_url = Url::parse(&url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_else(|| url.clone());
        // 百度广告走 baidu.php 跳转链（自然结果的 /link 不算），必须用归一化前的 href。
        let verdict = ads::classify(ads::AdEngine::Baidu, &title, &display_url, raw_href);
        if verdict.should_filter() {
            filtered_ads += 1;
            continue;
        }
        if !seen.insert(url.clone()) {
            continue;
        }
        results.push(SearchHit {
            rank: results.len() as u32 + 1,
            engine: SearchEngine::Baidu.as_str().to_owned(),
            title,
            url,
            display_url,
            snippet,
            engines: vec![SearchEngine::Baidu.as_str().to_owned()],
            is_ad: verdict.is_ad,
            same_domain_count: None,
            snippet_truncated: false,
        });
    }
    EngineSerp {
        results,
        related: Vec::new(),
        filtered_ads,
    }
}

/// 结果链接归一化：只保留 http(s)，丢弃百度跳转链与自身域名。
pub fn normalize_result_url(href: &str) -> Option<String> {
    let url = Url::parse(href).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if host == "baidu.com" || host.ends_with(".baidu.com") {
        return None;
    }
    let mut trimmed = url.clone();
    trimmed.set_fragment(None);
    Some(trimmed.to_string())
}

/// 取回并解析一次百度 SERP（带抖动 + 失败语义）。
pub async fn fetch_serp(
    base: &str,
    query: &str,
    page: usize,
    max_results: usize,
    exclude: &[String],
    jitter: &Jitter,
    fetcher: &PoliteFetcher,
) -> Envelope<EngineSerp> {
    let url = search_url(base, query, page, max_results, exclude);
    let fetched = fetch_html(&url, jitter, fetcher).await;
    finish(fetched, looks_blocked, parse_serp, max_results)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERP: &str = r#"<html><body><div id="content_left">
      <div class="result c-container" mu="https://example.com/a"><h3><a href="http://www.baidu.com/link?url=x">First</a></h3>
        <div class="c-abstract">First abstract</div></div>
      <div class="result c-container" mu="https://example.org/b"><h3><a href="http://www.baidu.com/link?url=y">Second</a></h3>
        <div class="c-abstract">Second abstract</div></div>
    </div></body></html>"#;

    #[test]
    fn parses_mu_attributes() {
        let serp = parse_serp(SERP, 10);
        assert_eq!(serp.results.len(), 2);
        assert_eq!(serp.results[0].url, "https://example.com/a");
        assert_eq!(serp.results[0].title, "First");
        assert_eq!(serp.results[0].snippet, "First abstract");
        assert_eq!(serp.results[0].engine, "baidu");
        assert_eq!(serp.results[0].display_url, "example.com");
    }

    #[test]
    fn skips_baidu_redirect_without_mu() {
        let html = r#"<div class="result"><h3><a href="http://www.baidu.com/link?url=z">Only link</a></h3></div>"#;
        assert!(parse_serp(html, 10).results.is_empty());
    }

    #[test]
    fn empty_page_is_empty() {
        assert!(parse_serp("<html></html>", 10).results.is_empty());
    }

    #[test]
    fn detects_blocked_markers() {
        assert!(looks_blocked("<div id=\"wappass\">安全验证</div>"));
        assert!(!looks_blocked("<html>ok</html>"));
    }

    #[test]
    fn url_paginates() {
        let url = search_url(DEFAULT_BASE, "rust 网页", 2, 10, &[]);
        assert!(url.contains("wd=rust+%E7%BD%91%E9%A1%B5"), "{url}");
        assert!(url.contains("pn=10"), "{url}");
        assert!(url.contains("rn=10"), "{url}");
    }
}
