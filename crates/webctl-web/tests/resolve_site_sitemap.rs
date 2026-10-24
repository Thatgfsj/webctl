//! L4（robots.txt 的 Sitemap 声明）离线集成测试：本地 fixture 冒充站点。
//!
//! 断言三态：声明（Declared）/ 可读但没声明（None）/ 拿不到（Unavailable）——
//! 三者必须分开，"拿不到 robots.txt"不得被说成"没声明"。

#[path = "support/http_fixture.rs"]
mod http_fixture;

use std::time::Duration;

use http_fixture::{Response, Server};
use webctl_web::politeness::{PoliteFetcher, PolitenessConfig};
use webctl_web::resolve_site::Candidate;
use webctl_web::resolve_site::sitemap::{self, SitemapStatus};
use webctl_web::transport::FetchOptions;

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

fn candidate(url: &str) -> Candidate {
    Candidate {
        url: url.to_owned(),
        score: 55,
        evidence: "直推".to_owned(),
        channel: "direct",
    }
}

/// 声明存在：如实报告，并保持 robots.txt 里的顺序。
#[tokio::test]
async fn declared_sitemaps_are_reported_in_file_order() {
    let server = Server::start_with(vec![(
        "/robots.txt",
        Response::text(
            "User-agent: *\nDisallow: /private\nSitemap: http://127.0.0.1/s1.xml\nSitemap: http://127.0.0.1/s2.xml\n",
        ),
    )])
    .await;
    let report = sitemap::probe(&server.url("/"), &fetcher()).await;
    assert_eq!(report.status, SitemapStatus::Declared);
    assert_eq!(
        report.sitemaps,
        vec![
            "http://127.0.0.1/s1.xml".to_owned(),
            "http://127.0.0.1/s2.xml".to_owned()
        ],
        "顺序必须与 robots.txt 一致"
    );
    assert_eq!(report.origin, server.base(), "origin 要能对上 fixture");
    assert_eq!(server.count("/robots.txt"), 1, "只抓一次 robots.txt");
}

/// 可读但没有 Sitemap 行：None（不是 Unavailable）。
#[tokio::test]
async fn robots_without_sitemap_is_none_not_unavailable() {
    let server = Server::start_with(vec![(
        "/robots.txt",
        Response::text("User-agent: *\nDisallow: /x\n"),
    )])
    .await;
    let report = sitemap::probe(&server.url("/"), &fetcher()).await;
    assert_eq!(report.status, SitemapStatus::None);
    assert!(report.sitemaps.is_empty());
}

/// robots.txt 拿不到（404）：Unavailable —— 不得当成"没声明"。
#[tokio::test]
async fn missing_robots_txt_is_unavailable() {
    let server = Server::start_with(Vec::<(String, Response)>::new()).await;
    let report = sitemap::probe(&server.url("/"), &fetcher()).await;
    assert_eq!(report.status, SitemapStatus::Unavailable);
}

/// probe_top 只探测上限内的候选，且候选 URL 原样回填、顺序不变。
#[tokio::test]
async fn probe_top_respects_limit_and_keeps_candidate_order() {
    let server = Server::start_with(vec![(
        "/robots.txt",
        Response::text("Sitemap: http://127.0.0.1/s.xml\n"),
    )])
    .await;
    let candidates = vec![
        candidate(&format!("{}/a", server.base())),
        candidate(&format!("{}/b", server.base())),
        candidate(&format!("{}/c", server.base())),
    ];
    let reports = sitemap::probe_top(&candidates, 2, &fetcher()).await;
    assert_eq!(reports.len(), 2, "上限 2 个候选");
    assert_eq!(reports[0].candidate_url, candidates[0].url);
    assert_eq!(reports[1].candidate_url, candidates[1].url);
}
