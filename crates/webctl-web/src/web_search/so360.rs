//! 360 搜索（so.com）SERP（HTML）适配（0.2.2 M3，议题 #202）。
//!
//! 实测（2026-11-01）：`https://www.so.com/s?q=…` 的自然结果是 `li.res-list`；
//! `h3.res-title a` 的 **`data-mdurl` 就是真实目标 URL**，`/link?m=` 是跳转链、丢弃；
//! 摘要在 `p.res-desc`，展示域名在 `cite`。

use scraper::{Html, Selector};
use url::Url;
use webctl_core::Envelope;

use crate::politeness::PoliteFetcher;

use super::ads;
use super::engine::{EngineSerp, contains_any, fetch_html, finish};
use super::{Jitter, SearchEngine, SearchHit};

/// 默认基址。
pub const DEFAULT_BASE: &str = "https://www.so.com/s";
/// 拦截特征（命中即 `search-blocked`）。
pub const BLOCKED_MARKERS: &[&str] = &["验证码", "captcha", "verify", "安全验证"];

/// 构造搜索 URL。
pub fn search_url(base: &str, query: &str, page: usize, _max_results: usize) -> String {
    let page = page.max(1);
    let encoded = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
    let join = if base.contains('?') { '&' } else { '?' };
    format!("{base}{join}q={encoded}&pn={page}")
}

/// 是否像拦截/验证码页。
pub fn looks_blocked(html: &str) -> bool {
    contains_any(html, BLOCKED_MARKERS)
}

/// 解析 360 SERP。
pub fn parse_serp(html: &str, max_results: usize) -> EngineSerp {
    let document = Html::parse_document(html);
    let block_selector = Selector::parse("li.res-list").expect("静态选择器");
    let title_selector = Selector::parse("h3.res-title").expect("静态选择器");
    let link_selector = Selector::parse("h3.res-title a[href]").expect("静态选择器");
    let desc_selector = Selector::parse("p.res-desc").expect("静态选择器");
    let cite_selector = Selector::parse("cite").expect("静态选择器");

    let mut results = Vec::new();
    let mut filtered_ads = 0usize;
    let mut seen = std::collections::HashSet::new();
    for block in document.select(&block_selector) {
        if results.len() >= max_results {
            break;
        }
        let Some(link) = block.select(&link_selector).next() else {
            continue;
        };
        let raw_href = link.value().attr("href").unwrap_or_default();
        let url = link
            .value()
            .attr("data-mdurl")
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
            .select(&desc_selector)
            .next()
            .map(|p| p.text().collect::<String>().trim().to_owned())
            .unwrap_or_default();
        let display_url = block
            .select(&cite_selector)
            .next()
            .map(|cite| cite.text().collect::<String>().trim().to_owned())
            .filter(|text| !text.is_empty())
            .or_else(|| {
                Url::parse(&url)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_owned))
            })
            .unwrap_or_else(|| url.clone());
        // 360 广告走 /Link（大写），自然结果的 /link（小写）不算，必须用归一化前的 href。
        let verdict = ads::classify(ads::AdEngine::So360, &title, &display_url, raw_href);
        if verdict.should_filter() {
            filtered_ads += 1;
            continue;
        }
        if !seen.insert(url.clone()) {
            continue;
        }
        results.push(SearchHit {
            rank: results.len() as u32 + 1,
            engine: SearchEngine::Sogou.as_str().to_owned(),
            title,
            url,
            display_url,
            snippet,
            engines: vec![SearchEngine::Sogou.as_str().to_owned()],
            is_ad: verdict.is_ad,
        });
    }
    EngineSerp {
        results,
        related: Vec::new(),
        filtered_ads,
    }
}

/// 结果链接归一化：只保留 http(s)，丢弃 360 跳转链与自身域名。
pub fn normalize_result_url(href: &str) -> Option<String> {
    let url = Url::parse(href).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if host == "so.com"
        || host.ends_with(".so.com")
        || host == "360.cn"
        || host.ends_with(".360.cn")
    {
        return None;
    }
    let mut trimmed = url.clone();
    trimmed.set_fragment(None);
    Some(trimmed.to_string())
}

/// 取回并解析一次 360 SERP（带抖动 + 失败语义）。
pub async fn fetch_serp(
    base: &str,
    query: &str,
    page: usize,
    max_results: usize,
    jitter: &Jitter,
    fetcher: &PoliteFetcher,
) -> Envelope<EngineSerp> {
    let url = search_url(base, query, page, max_results);
    let fetched = fetch_html(&url, jitter, fetcher).await;
    finish(fetched, looks_blocked, parse_serp, max_results)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERP: &str = r#"<html><body><ul class="result">
      <li class="res-list"><h3 class="res-title"><a data-mdurl="https://example.com/a" href="https://www.so.com/link?m=x">First</a></h3>
        <p class="res-desc">First desc</p><p class="g-linkinfo"><cite>example.com</cite></p></li>
      <li class="res-list"><h3 class="res-title"><a data-mdurl="https://example.org/b" href="https://www.so.com/link?m=y">Second</a></h3>
        <p class="res-desc">Second desc</p><p class="g-linkinfo"><cite>example.org</cite></p></li>
    </ul></body></html>"#;

    #[test]
    fn parses_data_mdurl() {
        let serp = parse_serp(SERP, 10);
        assert_eq!(serp.results.len(), 2);
        assert_eq!(serp.results[0].url, "https://example.com/a");
        assert_eq!(serp.results[0].engine, "360");
        assert_eq!(serp.results[0].snippet, "First desc");
        assert_eq!(serp.results[0].display_url, "example.com");
    }

    #[test]
    fn skips_so_redirect_without_mdurl() {
        let html = r#"<ul><li class="res-list"><h3 class="res-title"><a href="https://www.so.com/link?m=z">Only</a></h3></li></ul>"#;
        assert!(parse_serp(html, 10).results.is_empty());
    }

    #[test]
    fn detects_blocked_markers() {
        assert!(looks_blocked("<div>请输入验证码</div>"));
        assert!(!looks_blocked("<html>ok</html>"));
    }

    #[test]
    fn url_paginates() {
        let url = search_url(DEFAULT_BASE, "rust 网页", 3, 10);
        assert!(url.contains("q=rust+%E7%BD%91%E9%A1%B5"), "{url}");
        assert!(url.contains("pn=3"), "{url}");
    }
}
