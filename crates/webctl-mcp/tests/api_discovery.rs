//! #116：API 端点线索发现（只报告，不探测）。
//!
//! 验收要点：
//! - Next.js buildId、GraphQL、相对路径、跨域分组；
//! - 单次调用零额外网络请求（用 http_fixture 的计数断言）；
//! - 候选数 / URL 长度上限 + truncated。
//!
//! 全部用例只连 127.0.0.1 fixture；页面里出现的跨域端点只做字符串归一化，绝不请求。

mod common;

use common::{http_fixture, in_process_context};
use serde_json::{Value, json};
use webctl_mcp::WebctlMcp;

const PAGE: &str = r#"<html><head>
<script id="__NEXT_DATA__" type="application/json">{"buildId":"build-42","page":"/posts/7"}</script>
<script>
  fetch("/api/items?page=1");
  axios.post("/graphql", {query: "{ viewer { id } }"});
  fetch("https://api.elsewhere.test/v1/thing");
</script>
</head><body><div id="root"></div></body></html>"#;

async fn invoke(mcp: &WebctlMcp, args: Value) -> Value {
    mcp.invoke("embedded_data", args)
        .await
        .unwrap_or_else(|| panic!("embedded_data 未注册"))
}

/// 页面里写着端点，但单次调用只取页面本身：候选端点一个都不请求。
#[tokio::test]
async fn api_candidates_are_reported_without_extra_requests() {
    let server =
        http_fixture::Server::start_with(vec![("/page", http_fixture::Response::html(PAGE))]).await;
    let mcp = WebctlMcp::new(in_process_context("api-zero"));
    let envelope = invoke(&mcp, json!({"url": server.url("/page")})).await;

    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(
        server.total(),
        1,
        "单次调用只能取页面本身，不得探测候选端点：{envelope}"
    );
    assert_eq!(server.count("/api/items"), 0, "不得请求 /api/items");
    assert_eq!(server.count("/graphql"), 0, "不得请求 /graphql");

    let api = &envelope["data"]["api_candidates"];
    assert_eq!(api["truncated"], false, "{envelope}");
    assert_eq!(api["returned"], 4, "3 条同源 + 1 条跨域：{envelope}");

    let same = api["same_origin"].as_array().expect("same_origin");
    assert!(
        same.iter()
            .any(|endpoint| endpoint["url"] == server.url("/api/items?page=1")),
        "相对路径要归一化成绝对 URL：{envelope}"
    );
    assert!(
        same.iter()
            .any(|endpoint| endpoint["url"] == server.url("/graphql")
                && endpoint["kind"] == "graphql"),
        "GraphQL 端点要识别出 kind：{envelope}"
    );
    assert!(
        same.iter().any(|endpoint| endpoint["source"] == "next-data"
            && endpoint["url"] == server.url("/_next/data/build-42/posts/7.json")),
        "Next.js buildId 要合成 data 端点：{envelope}"
    );

    let cross = api["cross_origin"].as_array().expect("cross_origin");
    assert_eq!(cross.len(), 1, "跨域候选单独列出：{envelope}");
    assert_eq!(cross[0]["url"], "https://api.elsewhere.test/v1/thing");
    assert_eq!(cross[0]["same_origin"], false);

    assert!(
        api["note"].as_str().expect("note").contains("no request"),
        "输出必须写明没有发请求：{envelope}"
    );
}

/// html 输入 + base：相对路径归一化，且整个调用零网络请求。
#[tokio::test]
async fn html_input_uses_base_and_makes_no_requests() {
    let server = http_fixture::Server::start().await;
    let mcp = WebctlMcp::new(in_process_context("api-html"));
    let html = r#"<script>fetch("api/items");</script>"#;
    let envelope = invoke(&mcp, json!({"html": html, "base": server.url("/app/page")})).await;

    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(server.total(), 0, "html 输入不得产生任何网络请求");
    let same = envelope["data"]["api_candidates"]["same_origin"]
        .as_array()
        .expect("same_origin");
    assert_eq!(same.len(), 1, "{envelope}");
    assert_eq!(same[0]["url"], server.url("/app/api/items"));
}

/// 候选数上限：默认 32 条，超出部分截断并显式标记。
#[tokio::test]
async fn candidate_count_is_capped_and_truncated() {
    let mut html = String::from("<html><body>");
    for index in 0..40 {
        html.push_str(&format!(r#"<script>fetch("/api/items/{index}")</script>"#));
    }
    html.push_str("</body></html>");

    let mcp = WebctlMcp::new(in_process_context("api-cap"));
    let envelope = invoke(&mcp, json!({"html": html, "base": "https://example.test/"})).await;
    let api = &envelope["data"]["api_candidates"];
    assert_eq!(api["returned"], 32, "{envelope}");
    assert_eq!(api["candidates"], 40, "{envelope}");
    assert_eq!(api["dropped"], 8, "{envelope}");
    assert_eq!(api["truncated"], true, "{envelope}");
    assert_eq!(
        api["same_origin"].as_array().expect("same_origin").len(),
        32
    );
}

/// URL 长度上限：超长项不原样回显，只计数并置 truncated。
#[tokio::test]
async fn oversized_url_is_dropped_and_truncated() {
    let long = "a".repeat(2000);
    let html = format!(r#"<script>fetch("/api/{long}")</script>"#);
    let mcp = WebctlMcp::new(in_process_context("api-long"));
    let envelope = invoke(&mcp, json!({"html": html, "base": "https://example.test/"})).await;
    let api = &envelope["data"]["api_candidates"];
    assert_eq!(api["truncated"], true, "{envelope}");
    assert_eq!(api["returned"], 0, "{envelope}");
    assert_eq!(api["candidates"], 1, "{envelope}");
    assert_eq!(api["dropped"], 1, "{envelope}");
    assert!(
        envelope.to_string().len() < 10_000,
        "超长 URL 不得原样塞进输出：{envelope}"
    );
}
