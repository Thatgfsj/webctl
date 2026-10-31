//! `web_search`（0.2.2 M1，议题 #200）：离线 fixture 用例。
//!
//! 全部请求都打 127.0.0.1 fixture（`WEBCTL_SEARCH_BASE` 通过 McpContext 注入），零外网；
//! fixture 的 HTML 是**手工合成**的最小 Bing 结构，不含真实站点内容。

mod common;

use common::http_fixture;
use common::in_process_context;
use serde_json::{Value, json};
use webctl_mcp::WebctlMcp;

const SERP: &str = r#"<html><body><ol id="b_results">
  <li class="b_algo"><h2><a href="https://example.com/a">First result</a></h2>
    <div class="b_caption"><p>First snippet.</p></div><cite>example.com</cite></li>
  <li class="b_algo"><h2><a href="https://example.org/b">Second result</a></h2>
    <div class="b_caption"><p>Second snippet.</p></div><cite>example.org</cite></li>
  <li class="b_algo"><h2><a href="https://example.net/c">Third result</a></h2>
    <div class="b_caption"><p>Third snippet.</p></div><cite>example.net</cite></li>
</ol><div id="b_context"><a href="https://example.com/related">Related</a></div></body></html>"#;

const EMPTY: &str = r#"<html><body><ol id="b_results"></ol></body></html>"#;
const BLOCKED: &str =
    r#"<html><body><form class="challenge-form">unusual traffic</form></body></html>"#;

async fn call(mcp: &WebctlMcp, args: Value) -> Value {
    mcp.invoke("web_search", args)
        .await
        .unwrap_or_else(|| panic!("web_search 未注册"))
}

/// 默认 Bing：解析结果、逐引擎状态、合规 notes、只发 1 次请求且带抖动。
#[tokio::test]
async fn bing_results_are_parsed_and_disclosed() {
    let server =
        http_fixture::Server::start_with(vec![("/search", http_fixture::Response::html(SERP))])
            .await;
    let mcp = WebctlMcp::new(
        in_process_context("web-search-ok").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(&mcp, json!({"query": "rust web"})).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    let data = &envelope["data"];
    assert_eq!(data["results"].as_array().expect("results").len(), 3);
    assert_eq!(data["results"][0]["rank"], 1);
    assert_eq!(data["results"][0]["title"], "First result");
    assert_eq!(data["results"][0]["url"], "https://example.com/a");
    assert_eq!(data["results"][0]["engines"][0], "bing");
    assert_eq!(data["engines"][0]["engine"], "bing");
    assert_eq!(data["engines"][0]["status"], "hit");
    assert_eq!(data["engines"][0]["count"], 3);
    let notes = data["notes"].as_array().expect("notes");
    let joined = notes
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    assert!(joined.contains("robots"), "合规披露缺失：{joined}");
    assert!(joined.contains("不可信"), "不可信披露缺失：{joined}");
    // 默认只发 1 次请求（不抓正文），且 query/first 正确编码。
    let hits = server.hits("/search");
    assert_eq!(hits.len(), 1, "应只请求一次 SERP");
    let query = hits[0].query.clone().expect("query");
    assert!(
        query.contains("q=rust+web"),
        "query 应编码为表单风格：{query}"
    );
    assert!(query.contains("first=1"), "{query}");
    // 每次请求前 50–500ms 抖动：总耗时不可能低于 50ms。
    let elapsed = envelope["meta"]["elapsed_ms"].as_u64().expect("elapsed");
    assert!(elapsed >= 40, "抖动未生效？elapsed={elapsed}");
}

/// 第二页把 `first` 传给引擎。
#[tokio::test]
async fn pagination_sets_first_parameter() {
    let server =
        http_fixture::Server::start_with(vec![("/search", http_fixture::Response::html(SERP))])
            .await;
    let mcp = WebctlMcp::new(
        in_process_context("web-search-page").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(&mcp, json!({"query": "rust", "page": 2})).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    let query = server.hits("/search")[0].query.clone().expect("query");
    assert!(query.contains("first=11"), "{query}");
}

/// 验证码/拦截页 → DEGRADED + search-blocked + actionable。
#[tokio::test]
async fn blocked_page_degrades_with_actionable() {
    let server =
        http_fixture::Server::start_with(vec![("/search", http_fixture::Response::html(BLOCKED))])
            .await;
    let mcp = WebctlMcp::new(
        in_process_context("web-search-blocked").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(&mcp, json!({"query": "rust"})).await;
    assert_eq!(envelope["code"], "DEGRADED", "{envelope}");
    assert_eq!(envelope["meta"]["degraded_from"], "search-blocked");
    assert_eq!(envelope["data"]["engines"][0]["status"], "blocked");
    assert_eq!(
        envelope["meta"]["actionable"]["reason"],
        "search-unavailable"
    );
}

/// 零结果 → DEGRADED + search-empty（不返回 OK 空数组）。
#[tokio::test]
async fn empty_result_degrades_instead_of_ok() {
    let server =
        http_fixture::Server::start_with(vec![("/search", http_fixture::Response::html(EMPTY))])
            .await;
    let mcp = WebctlMcp::new(
        in_process_context("web-search-empty").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(&mcp, json!({"query": "nothing"})).await;
    assert_eq!(envelope["code"], "DEGRADED", "{envelope}");
    assert_eq!(envelope["meta"]["degraded_from"], "search-empty");
    assert_eq!(envelope["meta"]["actionable"]["reason"], "search-empty");
}

/// 未接线引擎 → UNSUPPORTED；未知引擎 / 空 query → PARSE。
#[tokio::test]
async fn invalid_engine_and_query_fail_explicitly() {
    let server =
        http_fixture::Server::start_with(vec![("/search", http_fixture::Response::html(SERP))])
            .await;
    let mcp = WebctlMcp::new(
        in_process_context("web-search-errors").with_search_base(Some(server.url("/search"))),
    );
    let unsupported = call(&mcp, json!({"query": "x", "engines": ["mojeek"]})).await;
    assert_eq!(unsupported["ok"], false, "{unsupported}");
    assert_eq!(unsupported["code"], "UNSUPPORTED");
    let unknown = call(&mcp, json!({"query": "x", "engines": ["google"]})).await;
    assert_eq!(unknown["code"], "PARSE");
    let empty = call(&mcp, json!({"query": "   "})).await;
    assert_eq!(empty["code"], "PARSE");
    assert_eq!(server.total(), 0, "参数错误不得发起网络请求");
}

/// max_results 有上限（30）且按引擎返回截断（不报错）。
#[tokio::test]
async fn max_results_is_clamped() {
    let server =
        http_fixture::Server::start_with(vec![("/search", http_fixture::Response::html(SERP))])
            .await;
    let mcp = WebctlMcp::new(
        in_process_context("web-search-max").with_search_base(Some(server.url("/search"))),
    );
    let two = call(&mcp, json!({"query": "rust", "max_results": 2})).await;
    assert_eq!(two["ok"], true, "{two}");
    assert_eq!(two["data"]["results"].as_array().expect("results").len(), 2);
    let many = call(&mcp, json!({"query": "rust", "max_results": 9999})).await;
    assert_eq!(many["ok"], true, "{many}");
    assert_eq!(
        many["data"]["results"].as_array().expect("results").len(),
        3
    );
}
