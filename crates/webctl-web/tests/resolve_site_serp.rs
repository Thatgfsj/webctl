//! L5（显式 opt-in 的 SERP 兜底）离线集成测试：本地 fixture 冒充搜索引擎结果页。
//!
//! 约束：测试里不得出现真实外站域名（offline lane 会扫描 /tests/），
//! 因此引擎域名用保留 TLD .test，结果链接用 example.*（保留域名）。

#[path = "support/http_fixture.rs"]
mod http_fixture;

use std::time::Duration;

use http_fixture::{Response, Server};
use webctl_web::politeness::{PoliteFetcher, PolitenessConfig};
use webctl_web::resolve_site::serp::{self, SCORE, SerpConfig, SerpFailure};
use webctl_web::transport::FetchOptions;

const RESULTS_HTML: &str = r#"<html><body>
<a href="https://example.com/">Example</a>
<a href="http://example.org/page/">Example Org</a>
<a href="http://engine.test/about">引擎自己的链接</a>
<a href="http://engine.test/l/?uddg=https%3A%2F%2Fexample.net%2F&rut=x">跳转链接</a>
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

fn config_for(server: &Server, max_results: usize) -> SerpConfig {
    SerpConfig {
        search_url: format!("{}/serp?q={{query}}", server.base()),
        query_suffix: " 官网".to_owned(),
        max_results,
        engine_host: "engine.test".to_owned(),
    }
}

/// 正常路径：结果变成低分候选（channel=serp-fallback、证据带名次），引擎自身链接被过滤、跳转链接被还原。
#[tokio::test]
async fn serp_results_become_low_score_candidates() {
    let server = Server::start_with(vec![("/serp", Response::html(RESULTS_HTML))]).await;
    let outcome = serp::search("某站", &config_for(&server, 5), &fetcher()).await;
    assert_eq!(outcome.failure, None, "{outcome:?}");
    let urls: Vec<&str> = outcome.candidates.iter().map(|c| c.url.as_str()).collect();
    assert_eq!(
        urls,
        vec![
            "https://example.com",
            "https://example.org/page",
            "https://example.net"
        ],
        "顺序 = 结果顺序；http 规范成 https、去尾斜杠；引擎链接被过滤；跳转链接被还原"
    );
    for (index, candidate) in outcome.candidates.iter().enumerate() {
        assert_eq!(candidate.score, SCORE);
        assert_eq!(candidate.channel, "serp-fallback");
        assert!(
            candidate.evidence.contains(&format!("第 {} 条", index + 1)),
            "证据要带名次：{}",
            candidate.evidence
        );
    }
}

/// 结果条数受 max_results 限制。
#[tokio::test]
async fn max_results_is_respected() {
    let server = Server::start_with(vec![("/serp", Response::html(RESULTS_HTML))]).await;
    let outcome = serp::search("某站", &config_for(&server, 1), &fetcher()).await;
    assert_eq!(outcome.candidates.len(), 1);
}

/// robots 禁止时**不请求**，并如实说"是 robots 拦的"。
#[tokio::test]
async fn robots_disallow_stops_before_requesting() {
    let server = Server::start_with(vec![
        (
            "/robots.txt",
            Response::text("User-agent: *\nDisallow: /serp\n"),
        ),
        ("/serp", Response::html(RESULTS_HTML)),
    ])
    .await;
    let outcome = serp::search("某站", &config_for(&server, 5), &fetcher()).await;
    assert_eq!(server.count("/serp"), 0, "robots 禁止时不得发 SERP 请求");
    match outcome.failure {
        Some(SerpFailure::Unavailable { code, message }) => {
            assert_eq!(code, "BLOCKED");
            assert!(
                message.contains("robots.txt"),
                "要说是 robots 拦的：{message}"
            );
        }
        other => panic!("期望 robots 阻断，实际 {other:?}"),
    }
}

/// 页面可读但没有结果：Empty（不是"解析失败"，也不是静默空候选）。
#[tokio::test]
async fn empty_page_is_empty_failure() {
    let server = Server::start_with(vec![(
        "/serp",
        Response::html("<html><body>无结果</body></html>"),
    )])
    .await;
    let outcome = serp::search("某站", &config_for(&server, 5), &fetcher()).await;
    assert_eq!(outcome.failure, Some(SerpFailure::Empty));
    assert!(outcome.candidates.is_empty());
}
