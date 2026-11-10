//! web_search 排除操作符（0.2.3 M2，议题 #222）：参数透传、回显与诚实降级。
//!
//! 全部请求打 127.0.0.1 的离线 fixture（with_search_base），零外网；
//! 生效校验只看本次返回结果，不额外发请求（用例里直接断言请求次数）。

mod common;

use common::http_fixture;
use common::in_process_context;
use serde_json::{Value, json};
use webctl_mcp::WebctlMcp;

const BING_SERP: &str = r#"<html><body><ol id="b_results">
  <li class="b_algo"><h2><a href="https://blog.csdn.net/post">Rust 教程</a></h2>
    <div class="b_caption"><p>CSDN 博客教程。</p></div><cite>blog.csdn.net</cite></li>
  <li class="b_algo"><h2><a href="https://example.com/docs">Rust 官方文档</a></h2>
    <div class="b_caption"><p>语言与标准库文档。</p></div><cite>example.com</cite></li>
</ol></body></html>"#;

const BAIDU_SERP: &str = r#"<html><body><div id="content_left">
  <div class="result c-container" mu="https://blog.csdn.net/post">
    <h3><a href="https://www.baidu.com/link?url=a">Rust 教程</a></h3>
    <div class="c-abstract">CSDN 博客教程。</div></div>
  <div class="result c-container" mu="https://example.com/docs">
    <h3><a href="https://www.baidu.com/link?url=b">Rust 官方文档</a></h3>
    <div class="c-abstract">语言与标准库文档。</div></div>
</div></body></html>"#;

async fn call(mcp: &WebctlMcp, args: Value) -> Value {
    mcp.invoke("web_search", args)
        .await
        .unwrap_or_else(|| panic!("web_search 未注册"))
}

async fn server_with(html: &'static str) -> http_fixture::Server {
    http_fixture::Server::start_with(vec![("/search", http_fixture::Response::html(html))]).await
}

/// 缺省 `-` 前缀被补齐、回显规范化形式，并且真的拼进引擎查询串。
#[tokio::test]
async fn exclude_is_normalized_reported_and_forwarded() {
    let server = server_with(BING_SERP).await;
    let mcp = WebctlMcp::new(
        in_process_context("exclude-forward").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(&mcp, json!({"query": "rust", "exclude": [" tutorial "]})).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["code"], "OK", "{envelope}");
    assert_eq!(envelope["data"]["applied_operators"], json!(["-tutorial"]));
    let query = server.hits("/search")[0].query.clone().expect("query");
    assert!(query.contains("q=rust+-tutorial"), "{query}");
    assert_eq!(server.hits("/search").len(), 1, "不得额外发请求");
}

/// Baidu 忽略 `-site:` 时：结果里仍有 csdn.net → DEGRADED + search-operator-ignored + actionable。
#[tokio::test]
async fn baidu_ignored_operator_degrades_with_actionable() {
    let server = server_with(BAIDU_SERP).await;
    let mcp = WebctlMcp::new(
        in_process_context("exclude-baidu").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(
        &mcp,
        json!({"query": "rust", "engines": ["baidu"], "exclude": ["-site:csdn.net"]}),
    )
    .await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["code"], "DEGRADED", "{envelope}");
    assert_eq!(envelope["meta"]["degraded_from"], "search-operator-ignored");
    assert_eq!(
        envelope["meta"]["actionable"]["reason"],
        "search-operator-ignored"
    );
    let steps = envelope["meta"]["actionable"]["next_steps"]
        .as_array()
        .expect("next_steps");
    assert!(
        steps.iter().any(|step| step == "fetch"),
        "建议通道必须真实：{steps:?}"
    );
    assert_eq!(
        envelope["data"]["applied_operators"],
        json!(["-site:csdn.net"])
    );
    assert_eq!(server.hits("/search").len(), 1, "校验不得额外发请求");
}

/// 部分引擎失败与操作符被忽略同时成立 → 逗号组合的 degraded_from。
#[tokio::test]
async fn operator_ignore_combines_with_partial_engine_failure() {
    let server = server_with(BING_SERP).await;
    let mcp = WebctlMcp::new(
        in_process_context("exclude-partial").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(
        &mcp,
        json!({
            "query": "rust",
            "engines": ["bing", "mojeek"],
            "exclude": ["-site:csdn.net"]
        }),
    )
    .await;
    assert_eq!(envelope["code"], "DEGRADED", "{envelope}");
    assert_eq!(
        envelope["meta"]["degraded_from"],
        "search-partial,search-operator-ignored"
    );
    assert_eq!(server.hits("/search").len(), 1, "未接线引擎不得发请求");
}

/// 不带 exclude：行为与 0.2.2 一致（OK，且没有 applied_operators 字段）。
#[tokio::test]
async fn without_exclude_the_path_is_unchanged() {
    let server = server_with(BING_SERP).await;
    let mcp = WebctlMcp::new(
        in_process_context("exclude-absent").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(&mcp, json!({"query": "rust"})).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["code"], "OK", "{envelope}");
    assert!(
        envelope["data"].get("applied_operators").is_none(),
        "未传 exclude 不应回显空的 applied_operators：{envelope}"
    );
    assert!(envelope["meta"]["degraded_from"].is_null(), "{envelope}");
}
