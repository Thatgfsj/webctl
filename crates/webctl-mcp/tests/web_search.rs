//! `web_search`（0.2.2 M1 + M3，议题 #200 / #202）：离线 fixture 用例。
//!
//! 全部请求都打 127.0.0.1 fixture（`with_search_base` 注入，覆盖所有已接线引擎），零外网；
//! fixture 的 HTML 是**手工合成**的最小结构，不含真实站点内容。

mod common;

use common::http_fixture;
use common::in_process_context;
use serde_json::{Value, json};
use webctl_mcp::WebctlMcp;

const BING_SERP: &str = r#"<html><body><ol id="b_results">
  <li class="b_algo"><h2><a href="https://example.com/shared">Bing shared</a></h2>
    <div class="b_caption"><p>Bing snippet.</p></div><cite>example.com</cite></li>
  <li class="b_algo"><h2><a href="https://example.com/only-bing">Bing only</a></h2>
    <div class="b_caption"><p>Bing only snippet.</p></div><cite>example.com</cite></li>
</ol><div id="b_context"><a href="https://example.com/related">Related</a></div></body></html>"#;

const COMBINED: &str = r#"<html><body>
<ol id="b_results"><li class="b_algo"><h2><a href="https://example.com/shared">Bing shared</a></h2>
  <div class="b_caption"><p>Bing snippet.</p></div><cite>example.com</cite></li>
<li class="b_algo"><h2><a href="https://example.com/only-bing">Bing only</a></h2></li></ol>
<div id="content_left"><div class="result c-container" mu="https://example.com/shared">
  <h3><a>Baidu shared</a></h3><div class="c-abstract">Baidu snippet.</div></div></div>
<ul class="result"><li class="res-list"><h3 class="res-title">
  <a data-mdurl="https://example.com/only-360" href="https://example.com/wrapper?m=x">360 only</a></h3>
  <p class="res-desc">360 snippet.</p></li></ul>
</body></html>"#;

const EMPTY: &str = r#"<html><body><ol id="b_results"></ol></body></html>"#;
const BLOCKED: &str =
    r#"<html><body><form class="challenge-form">unusual traffic</form></body></html>"#;

async fn call(mcp: &WebctlMcp, args: Value) -> Value {
    mcp.invoke("web_search", args)
        .await
        .unwrap_or_else(|| panic!("web_search 未注册"))
}

fn notes_text(envelope: &Value) -> String {
    envelope["data"]["notes"]
        .as_array()
        .map(|notes| {
            notes
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

/// 默认 Bing：解析结果、逐引擎状态、合规 notes、只发 1 次请求且带抖动。
#[tokio::test]
async fn bing_results_are_parsed_and_disclosed() {
    let server = http_fixture::Server::start_with(vec![(
        "/search",
        http_fixture::Response::html(BING_SERP),
    )])
    .await;
    let mcp = WebctlMcp::new(
        in_process_context("web-search-ok").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(&mcp, json!({"query": "rust web"})).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    let data = &envelope["data"];
    assert_eq!(data["results"].as_array().expect("results").len(), 2);
    assert_eq!(data["results"][0]["rank"], 1);
    assert_eq!(data["results"][0]["engines"][0], "bing");
    assert_eq!(data["engines"][0]["status"], "hit");
    let notes = notes_text(&envelope);
    assert!(notes.contains("robots"), "合规披露缺失：{notes}");
    assert!(notes.contains("不可信"), "不可信披露缺失：{notes}");
    let hits = server.hits("/search");
    assert_eq!(hits.len(), 1, "应只请求一次 SERP");
    let query = hits[0].query.clone().expect("query");
    assert!(query.contains("q=rust+web"), "{query}");
    assert!(query.contains("first=1"), "{query}");
    let elapsed = envelope["meta"]["elapsed_ms"].as_u64().expect("elapsed");
    assert!(elapsed >= 40, "抖动未生效？elapsed={elapsed}");
}

/// 第二页把 first/pn 传给引擎；page 上限 5。
#[tokio::test]
async fn pagination_sets_first_and_caps_page() {
    let server = http_fixture::Server::start_with(vec![(
        "/search",
        http_fixture::Response::html(BING_SERP),
    )])
    .await;
    let mcp = WebctlMcp::new(
        in_process_context("web-search-page").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(&mcp, json!({"query": "rust", "page": 2})).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    let first = server.hits("/search")[0].query.clone().expect("query");
    assert!(first.contains("first=11"), "{first}");
    let capped = call(&mcp, json!({"query": "rust", "page": 9})).await;
    assert_eq!(capped["ok"], true, "{capped}");
    let last = server.hits("/search").last().cloned().expect("hit");
    assert!(last.query.unwrap().contains("first=41"), "page 应被压到 5");
}

/// 拦截页（单引擎）→ DEGRADED + search-blocked + actionable。
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

/// 零结果（单引擎）→ DEGRADED + search-empty。
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

/// 多引擎：3 次请求、去重（同一 URL 保留两引擎证据）、RRF 排序、逐引擎状态。
#[tokio::test]
async fn multi_engine_merges_and_dedupes() {
    let server =
        http_fixture::Server::start_with(vec![("/search", http_fixture::Response::html(COMBINED))])
            .await;
    let mcp = WebctlMcp::new(
        in_process_context("web-search-multi").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(
        &mcp,
        json!({"query": "rust", "engines": ["bing", "baidu", "360"]}),
    )
    .await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    let data = &envelope["data"];
    assert_eq!(server.hits("/search").len(), 3, "三个引擎各一次请求");
    assert_eq!(data["engines"].as_array().expect("engines").len(), 3);
    for status in data["engines"].as_array().expect("engines") {
        assert_eq!(status["status"], "hit", "{status}");
    }
    let results = data["results"].as_array().expect("results");
    let shared = results
        .iter()
        .find(|hit| hit["url"] == "https://example.com/shared")
        .expect("重复 URL 应保留一条");
    assert_eq!(shared["engines"].as_array().expect("evidence").len(), 2);
    assert_eq!(
        results
            .iter()
            .filter(|hit| hit["url"] == "https://example.com/shared")
            .count(),
        1,
        "同 URL 必须去重"
    );
}

/// 未接线引擎：如实报 unavailable、**不发请求**；全失败 → search-all-failed。
#[tokio::test]
async fn unwired_engine_reports_unavailable_without_request() {
    let server = http_fixture::Server::start_with(vec![(
        "/search",
        http_fixture::Response::html(BING_SERP),
    )])
    .await;
    let mcp = WebctlMcp::new(
        in_process_context("web-search-unwired").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(&mcp, json!({"query": "rust", "engines": ["mojeek"]})).await;
    assert_eq!(envelope["code"], "DEGRADED", "{envelope}");
    assert_eq!(envelope["meta"]["degraded_from"], "search-all-failed");
    assert_eq!(envelope["data"]["engines"][0]["status"], "unavailable");
    assert_eq!(envelope["data"]["engines"][0]["count"], 0);
    assert_eq!(server.total(), 0, "未接线引擎不得发起请求");
}

/// 部分失败：bing 成功 + mojeek 未接线 → DEGRADED + search-partial（结果仍返回）。
#[tokio::test]
async fn partial_failure_degrades_with_partial() {
    let server = http_fixture::Server::start_with(vec![(
        "/search",
        http_fixture::Response::html(BING_SERP),
    )])
    .await;
    let mcp = WebctlMcp::new(
        in_process_context("web-search-partial").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(
        &mcp,
        json!({"query": "rust", "engines": ["bing", "mojeek"]}),
    )
    .await;
    assert_eq!(envelope["code"], "DEGRADED", "{envelope}");
    assert_eq!(envelope["meta"]["degraded_from"], "search-partial");
    assert_eq!(envelope["meta"]["actionable"]["reason"], "search-partial");
    assert_eq!(
        envelope["data"]["results"]
            .as_array()
            .expect("results")
            .len(),
        2
    );
    let statuses = envelope["data"]["engines"].as_array().expect("engines");
    assert_eq!(statuses[0]["status"], "hit");
    assert_eq!(statuses[1]["status"], "unavailable");
    assert_eq!(server.hits("/search").len(), 1);
}

/// 单引擎硬失败（HTTP 500）→ 原样 fail(NETWORK)，不得伪装成 DEGRADED/search-all-failed。
#[tokio::test]
async fn single_engine_hard_failure_is_not_disguised() {
    // 回环 + 关闭端口：连接被拒（NETWORK），不依赖任何站点。
    let mcp = WebctlMcp::new(
        in_process_context("web-search-hard-fail")
            .with_search_base(Some("http://127.0.0.1:1/search".to_owned())),
    );
    let envelope = call(&mcp, json!({"query": "rust"})).await;
    assert_eq!(envelope["ok"], false, "{envelope}");
    assert_eq!(envelope["code"], "NETWORK", "{envelope}");
    assert_ne!(envelope["meta"]["degraded_from"], "search-all-failed");
}

/// 多引擎硬失败 → DEGRADED + search-all-failed，逐引擎状态为 failed（不是 hit）。
#[tokio::test]
async fn multi_engine_hard_failure_reports_failed_statuses() {
    let mcp = WebctlMcp::new(
        in_process_context("web-search-multi-fail")
            .with_search_base(Some("http://127.0.0.1:1/search".to_owned())),
    );
    let envelope = call(&mcp, json!({"query": "rust", "engines": ["bing", "baidu"]})).await;
    assert_eq!(envelope["code"], "DEGRADED", "{envelope}");
    assert_eq!(envelope["meta"]["degraded_from"], "search-all-failed");
    for status in envelope["data"]["engines"].as_array().expect("engines") {
        let value = status["status"].as_str().expect("status");
        assert!(
            matches!(value, "failed" | "error"),
            "逐引擎状态必须如实（failed/error）：{status}"
        );
        assert!(
            status["note"].as_str().is_some_and(|note| !note.is_empty()),
            "失败原因不能为空：{status}"
        );
    }
}

/// Baidu 第二页把 pn 传给引擎。
#[tokio::test]
async fn baidu_pagination_sets_pn() {
    let server =
        http_fixture::Server::start_with(vec![("/search", http_fixture::Response::html(COMBINED))])
            .await;
    let mcp = WebctlMcp::new(
        in_process_context("web-search-baidu-page").with_search_base(Some(server.url("/search"))),
    );
    let _ = call(
        &mcp,
        json!({"query": "rust", "engines": ["baidu"], "page": 2}),
    )
    .await;
    let query = server.hits("/search")[0].query.clone().expect("query");
    assert!(query.contains("pn=10"), "{query}");
}

/// 参数错误：未知引擎 / 空 query → PARSE，且不联网。
#[tokio::test]
async fn invalid_query_and_engine_fail_without_network() {
    let server = http_fixture::Server::start_with(vec![(
        "/search",
        http_fixture::Response::html(BING_SERP),
    )])
    .await;
    let mcp = WebctlMcp::new(
        in_process_context("web-search-errors").with_search_base(Some(server.url("/search"))),
    );
    let unknown = call(&mcp, json!({"query": "x", "engines": ["google"]})).await;
    assert_eq!(unknown["code"], "PARSE");
    let empty = call(&mcp, json!({"query": "   "})).await;
    assert_eq!(empty["code"], "PARSE");
    assert_eq!(server.total(), 0, "参数错误不得发起网络请求");
}

/// max_results 有上限（30）且按引擎返回截断（不报错）。
#[tokio::test]
async fn max_results_is_clamped() {
    let server = http_fixture::Server::start_with(vec![(
        "/search",
        http_fixture::Response::html(BING_SERP),
    )])
    .await;
    let mcp = WebctlMcp::new(
        in_process_context("web-search-max").with_search_base(Some(server.url("/search"))),
    );
    let one = call(&mcp, json!({"query": "rust", "max_results": 1})).await;
    assert_eq!(one["ok"], true, "{one}");
    assert_eq!(one["data"]["results"].as_array().expect("results").len(), 1);
    let many = call(&mcp, json!({"query": "rust", "max_results": 9999})).await;
    assert_eq!(many["ok"], true, "{many}");
    assert_eq!(
        many["data"]["results"].as_array().expect("results").len(),
        2
    );
}
