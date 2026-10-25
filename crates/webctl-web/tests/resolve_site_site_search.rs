//! L2（站点内建搜索）离线集成测试：本地 fixture 冒充"站点的搜索页"。
//!
//! 断言：只收"名称匹配的外部链接"（内部链接与非匹配结果都过滤）、逐站点报告三态、
//! robots 拦下不请求、探测站点数受上限约束。

#[path = "support/http_fixture.rs"]
mod http_fixture;

use std::time::Duration;

use http_fixture::{Response, Server};
use webctl_web::politeness::{PoliteFetcher, PolitenessConfig};
use webctl_web::resolve_site::site_search::{
    self, SCORE_CORROBORATED, SCORE_SINGLE, SearchTemplate, SiteSearchConfig, SiteSearchStatus,
};
use webctl_web::transport::FetchOptions;

const SEARCH_HTML: &str = r#"<html><body>
<a href="/internal/page">Example 站内页面</a>
<a href="https://example.com/">Example 官网</a>
<a href="https://unrelated.test/thing">完全无关的结果</a>
<a href="http://example.org/">Example Org 官网</a>
</body></html>"#;

fn fetcher() -> PoliteFetcher {
    PoliteFetcher::new(
        PolitenessConfig {
            max_retries: 0,
            ..PolitenessConfig::default()
        },
        FetchOptions {
            timeout: Duration::from_secs(5),
            ..FetchOptions::default()
        },
    )
}

fn config_for(server: &Server, max_sites: usize) -> SiteSearchConfig {
    SiteSearchConfig {
        templates: vec![SearchTemplate {
            site: "fixture-search",
            url: Box::leak(format!("{}/search?q={{query}}", server.base()).into_boxed_str()),
        }],
        max_sites,
        max_results: 10,
    }
}

/// 正常路径：名称匹配的外部链接成为候选（单站点命中 55 分、site-search 通道），内部/无关链接被过滤。
#[tokio::test]
async fn matching_external_links_become_candidates() {
    let server = Server::start_with(vec![("/search", Response::html(SEARCH_HTML))]).await;
    let outcome = site_search::search("Example", &config_for(&server, 2), &fetcher()).await;
    let urls: Vec<&str> = outcome.candidates.iter().map(|c| c.url.as_str()).collect();
    assert_eq!(
        urls,
        vec!["https://example.com", "https://example.org"],
        "内部链接与无关结果必须被过滤"
    );
    for candidate in &outcome.candidates {
        assert_eq!(candidate.score, SCORE_SINGLE);
        assert_eq!(candidate.channel, "site-search");
        assert!(candidate.evidence.contains("fixture-search"));
    }
    assert_eq!(outcome.reports.len(), 1);
    assert_eq!(outcome.reports[0].status, SiteSearchStatus::Hit);
    assert_eq!(outcome.reports[0].candidates, 2);
}

/// robots 拦下：**不请求**，报告为 Unavailable 且 note 保留 robots 原因。
#[tokio::test]
async fn robots_disallow_is_reported_per_site() {
    let server = Server::start_with(vec![
        (
            "/robots.txt",
            Response::text("User-agent: *\nDisallow: /search\n"),
        ),
        ("/search", Response::html(SEARCH_HTML)),
    ])
    .await;
    let outcome = site_search::search("Example", &config_for(&server, 2), &fetcher()).await;
    assert_eq!(server.count("/search"), 0, "robots 禁止时不得发搜索请求");
    assert_eq!(outcome.reports.len(), 1);
    assert_eq!(outcome.reports[0].status, SiteSearchStatus::Unavailable);
    assert!(
        outcome.reports[0].note.contains("robots.txt"),
        "note 要保留 robots 原因：{}",
        outcome.reports[0].note
    );
    assert!(outcome.candidates.is_empty());
}

/// 页面可读但没有匹配结果：Empty（不是 Unavailable）。
#[tokio::test]
async fn page_without_matches_is_empty() {
    let server = Server::start_with(vec![(
        "/search",
        Response::html(r#"<html><body><a href="https://unrelated.test/">无关</a></body></html>"#),
    )])
    .await;
    let outcome = site_search::search("Example", &config_for(&server, 2), &fetcher()).await;
    assert_eq!(outcome.reports[0].status, SiteSearchStatus::Empty);
    assert!(outcome.candidates.is_empty());
}

/// max_sites 限制探测站点数：给了 2 个模板、上限 1 → 只探测第一个。
#[tokio::test]
async fn max_sites_limits_probing() {
    let server = Server::start_with(vec![
        ("/search", Response::html(SEARCH_HTML)),
        ("/search2", Response::html(SEARCH_HTML)),
    ])
    .await;
    let first = format!("{}/search?q={{query}}", server.base());
    let second = format!("{}/search2?q={{query}}", server.base());
    let config = SiteSearchConfig {
        templates: vec![
            SearchTemplate {
                site: "first",
                url: Box::leak(first.into_boxed_str()),
            },
            SearchTemplate {
                site: "second",
                url: Box::leak(second.into_boxed_str()),
            },
        ],
        max_sites: 1,
        max_results: 10,
    };
    let outcome = site_search::search("Example", &config, &fetcher()).await;
    assert_eq!(outcome.reports.len(), 1);
    assert_eq!(outcome.reports[0].site, "first");
    assert_eq!(server.count("/search2"), 0, "上限 1 时不得探测第二个模板");
}

/// 两个独立模板都命中：分值升到互证档（仍低于 60），证据里说明互证。
#[tokio::test]
async fn two_sites_hitting_raises_corroborated_score() {
    let server = Server::start_with(vec![
        ("/search", Response::html(SEARCH_HTML)),
        ("/search2", Response::html(SEARCH_HTML)),
    ])
    .await;
    let first = format!("{}/search?q={{query}}", server.base());
    let second = format!("{}/search2?q={{query}}", server.base());
    let config = SiteSearchConfig {
        templates: vec![
            SearchTemplate {
                site: "first",
                url: Box::leak(first.into_boxed_str()),
            },
            SearchTemplate {
                site: "second",
                url: Box::leak(second.into_boxed_str()),
            },
        ],
        max_sites: 2,
        max_results: 10,
    };
    let outcome = site_search::search("Example", &config, &fetcher()).await;
    assert_eq!(outcome.reports.len(), 2);
    assert!(outcome.candidates.len() >= 2);
    for candidate in &outcome.candidates {
        assert_eq!(candidate.score, SCORE_CORROBORATED);
        assert!(
            candidate.evidence.contains("≥2 个站点命中"),
            "证据要说明是站点维度的互证：{}",
            candidate.evidence
        );
        assert!(candidate.score < 60, "互证也不能跨可靠阈值");
    }
    // 逐站点报告必须自洽：第二个站点命中的候选可能全被去重，但 note 不能说"没有匹配"
    // （回归：曾用去重后的计数拼 note，导致 status=Hit 而 note 说无匹配）。
    assert_eq!(outcome.reports[1].status, SiteSearchStatus::Hit);
    assert_eq!(outcome.reports[1].candidates, 2);
    assert!(
        outcome.reports[1].note.contains("2 条匹配的外部链接"),
        "命中站点的 note 必须与 status/candidates 一致：{}",
        outcome.reports[1].note
    );
}
