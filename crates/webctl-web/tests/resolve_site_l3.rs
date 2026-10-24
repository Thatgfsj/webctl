//! L3（Wikidata P856）离线集成测试：用本地 fixture 冒充 Wikidata API。
//!
//! 为什么要 fixture：生产端点在外网，测试若直连会变成联网用例；这里注入端点，
//! 让"请求形状 / 解析 / robots 阻断 / 显式失败"都能被钉住，且只访问 127.0.0.1。

#[path = "support/http_fixture.rs"]
mod http_fixture;

use std::time::Duration;

use http_fixture::{Response, Server};
use webctl_web::politeness::{PoliteFetcher, PolitenessConfig};
use webctl_web::resolve_site::wikidata::{self, L3Config, L3Failure, SCORE};
use webctl_web::transport::FetchOptions;

/// 指向本地 fixture 的端点（真实默认端点见 L3Config::default）。
fn config_for(server: &Server) -> L3Config {
    L3Config {
        search_url: format!(
            "{}/wbsearch?search={{query}}&lang={{lang}}&limit={{limit}}",
            server.base()
        ),
        entities_url: format!("{}/wbentities?ids={{id}}", server.base()),
        language: "zh".to_owned(),
        limit: 3,
    }
}

/// 不重试、短超时的礼貌取回器（测试要快，且失败路径不该被重试掩盖）。
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

fn search_json(id: &str, label: &str) -> Response {
    Response::json(format!(
        r#"{{"search":[{{"id":"{id}","label":"{label}","description":"d"}}]}}"#
    ))
}

fn entities_with_p856(url: &str) -> Response {
    Response::json(format!(
        r#"{{"entities":{{"Q123":{{"claims":{{"P856":[{{"mainsnak":{{"datavalue":{{"value":"{url}"}}}}}}]}}}}}}}}"#
    ))
}

/// 正常路径：搜索命中 → 取 P856 → 给出 public-api 候选（https 规范化）。
#[tokio::test]
async fn p856_becomes_official_site_candidate() {
    let server = Server::start_with(vec![
        ("/wbsearch", search_json("Q123", "某站")),
        ("/wbentities", entities_with_p856("http://example.com/")),
    ])
    .await;
    let outcome = wikidata::resolve("某站", &config_for(&server), &fetcher()).await;
    assert_eq!(outcome.failure, None, "{outcome:?}");
    assert_eq!(outcome.qid.as_deref(), Some("Q123"));
    assert_eq!(outcome.candidates.len(), 1);
    let candidate = &outcome.candidates[0];
    assert_eq!(
        candidate.url, "https://example.com",
        "http 要规范成 https、去尾斜杠"
    );
    assert_eq!(candidate.score, SCORE);
    assert_eq!(candidate.channel, "public-api");
    assert!(
        candidate.evidence.contains("Q123") && candidate.evidence.contains("某站"),
        "证据要能复核：{}",
        candidate.evidence
    );
    // 请求形状：两次请求、第二次带 Q 号。
    assert_eq!(server.count("/wbsearch"), 1);
    assert_eq!(server.count("/wbentities"), 1);
}

/// robots 拦下时**不请求**，并如实说"是 robots 拦的"。
#[tokio::test]
async fn robots_disallow_stops_before_requesting() {
    let server = Server::start_with(vec![
        (
            "/robots.txt",
            Response::text("User-agent: *\nDisallow: /wbsearch\n"),
        ),
        ("/wbsearch", search_json("Q123", "某站")),
        ("/wbentities", entities_with_p856("https://example.com")),
    ])
    .await;
    let outcome = wikidata::resolve("某站", &config_for(&server), &fetcher()).await;
    assert_eq!(
        server.count("/wbsearch"),
        0,
        "robots 禁止时不得发出搜索请求"
    );
    match outcome.failure {
        Some(L3Failure::SearchUnavailable { code, message }) => {
            assert_eq!(code, "BLOCKED");
            assert!(
                message.contains("robots.txt"),
                "要说是 robots 拦的：{message}"
            );
        }
        other => panic!("期望 robots 阻断，实际 {other:?}"),
    }
}

/// 搜索无命中：NoMatch（不是"没找到所以返回空数组"那种静默）。
#[tokio::test]
async fn empty_search_is_no_match() {
    let server = Server::start_with(vec![("/wbsearch", Response::json(r#"{"search":[]}"#))]).await;
    let outcome = wikidata::resolve("查无此站", &config_for(&server), &fetcher()).await;
    assert_eq!(outcome.failure, Some(L3Failure::NoMatch));
    assert!(outcome.candidates.is_empty());
}

/// 命中实体但没有 P856：NoOfficialSite（如实区分"没声明官网"）。
#[tokio::test]
async fn entity_without_p856_is_no_official_site() {
    let server = Server::start_with(vec![
        ("/wbsearch", search_json("Q123", "某站")),
        (
            "/wbentities",
            Response::json(r#"{"entities":{"Q123":{"claims":{"P31":[]}}}}"#),
        ),
    ])
    .await;
    let outcome = wikidata::resolve("某站", &config_for(&server), &fetcher()).await;
    assert_eq!(outcome.failure, Some(L3Failure::NoOfficialSite));
    assert_eq!(outcome.qid.as_deref(), Some("Q123"));
}

/// 响应不是 JSON：SearchParse（不清空成"没找到"）。
#[tokio::test]
async fn non_json_search_is_parse_failure() {
    let server = Server::start_with(vec![("/wbsearch", Response::text("not json"))]).await;
    let outcome = wikidata::resolve("某站", &config_for(&server), &fetcher()).await;
    match outcome.failure {
        Some(L3Failure::SearchParse(detail)) => assert!(!detail.is_empty()),
        other => panic!("期望 SearchParse，实际 {other:?}"),
    }
}

/// 实体请求 500：EntitiesUnavailable 且带码（不是静默空结果）。
#[tokio::test]
async fn entities_error_is_reported_with_code() {
    let server = Server::start_with(vec![
        ("/wbsearch", search_json("Q123", "某站")),
        ("/wbentities", Response::new(500, "boom")),
    ])
    .await;
    let outcome = wikidata::resolve("某站", &config_for(&server), &fetcher()).await;
    match outcome.failure {
        Some(L3Failure::EntitiesUnavailable { code, message }) => {
            assert_eq!(code, "BLOCKED");
            assert!(message.contains("500"), "{message}");
        }
        other => panic!("期望 EntitiesUnavailable，实际 {other:?}"),
    }
}
