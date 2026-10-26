//! #125：以 URL 为输入的三个工具（doc_text / json_query / download）必须继承上游降级。
//!
//! 背景：fetch 在 JS 空壳页上已经给出 DEGRADED + degraded_from=js-render + meta.actionable；
//! 但这些工具自己重建信封、只拿上游文本，若丢掉 meta 就会静默返回 OK（违反 A3）。
//! 反向要求同样重要：本工具自己的失败（json_query 解析失败）不得被伪装成降级，
//! 静态页也不得被无端标降级（否则降级信号贬值）。

mod common;

use common::http_fixture;
use common::{JS_SHELL, STATIC_QUOTES, in_process_context, temp_dir};
use serde_json::{Value, json};
use webctl_mcp::WebctlMcp;

async fn invoke(mcp: &WebctlMcp, name: &str, args: Value) -> Value {
    mcp.invoke(name, args)
        .await
        .unwrap_or_else(|| panic!("工具 {name} 未注册"))
}

fn code_of(envelope: &Value) -> &str {
    envelope["code"].as_str().unwrap_or("<missing>")
}

/// 断言信封是「显式降级」：ok=true（data 仍可用）+ code 非 OK + degraded_from 非空。
fn assert_degraded(envelope: &Value, context: &str) {
    assert_eq!(
        envelope["ok"], true,
        "{context}: 降级不是失败，data 仍须可用：{envelope}"
    );
    assert_ne!(
        code_of(envelope),
        "OK",
        "{context}: 禁止静默返回 OK：{envelope}"
    );
    assert!(
        envelope["meta"]["degraded_from"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "{context}: 降级必须写明 degraded_from：{envelope}"
    );
}

/// 降级信封的 actionable 必须完整，且建议通道是真实工具（#115 / #125）。
fn assert_actionable_channels(envelope: &Value, context: &str) {
    let actionable = &envelope["meta"]["actionable"];
    assert!(
        actionable.is_object(),
        "{context}: 降级必须带 meta.actionable：{envelope}"
    );
    assert!(
        actionable["reason"].as_str().is_some_and(|s| !s.is_empty()),
        "{context}: 原因码不得为空：{actionable}"
    );
    for step in actionable["next_steps"].as_array().expect("next_steps") {
        let name = step.as_str().expect("channel");
        assert!(
            webctl_mcp::dispatch::is_known_tool(name),
            "{context}: 通道 {name} 必须是真实工具"
        );
    }
}

/// doc_text 以 URL 为输入时继承上游降级，不得在 JS 空壳页上静默返回 OK。
#[tokio::test]
async fn doc_text_inherits_upstream_js_render_degradation() {
    let server =
        http_fixture::Server::start_with(vec![("/js", http_fixture::Response::html(JS_SHELL))])
            .await;
    let mcp = WebctlMcp::new(in_process_context("doc-text-inherit"));
    let doc = invoke(&mcp, "doc_text", json!({"url": server.url("/js")})).await;
    assert_degraded(&doc, "doc_text /js");
    assert_eq!(
        doc["meta"]["degraded_from"], "js-render",
        "必须写明降级来源：{doc}"
    );
    assert!(
        !doc["data"].is_null(),
        "降级不是失败：文本结果仍要返回：{doc}"
    );
    assert_actionable_channels(&doc, "doc_text /js");
}

/// 反向守卫：静态页不受影响（降级信号不得贬值成「总是降级」）。
#[tokio::test]
async fn doc_text_on_static_page_stays_ok() {
    let server = http_fixture::Server::start_with(vec![(
        "/static",
        http_fixture::Response::html(STATIC_QUOTES),
    )])
    .await;
    let mcp = WebctlMcp::new(in_process_context("doc-text-static"));
    let doc = invoke(&mcp, "doc_text", json!({"url": server.url("/static")})).await;
    assert_eq!(code_of(&doc), "OK", "静态页必须仍是 OK：{doc}");
    assert!(
        doc["meta"]["degraded_from"].is_null(),
        "静态页不得带 degraded_from：{doc}"
    );
}

/// download 同样继承上游降级（且文件确实落盘：降级不代表下载失败）。
#[tokio::test]
async fn download_inherits_upstream_js_render_degradation() {
    let server =
        http_fixture::Server::start_with(vec![("/js", http_fixture::Response::html(JS_SHELL))])
            .await;
    let dir = temp_dir("download-inherit");
    let mcp = WebctlMcp::new(in_process_context("download-inherit"));
    let saved = invoke(
        &mcp,
        "download",
        json!({"url": server.url("/js"), "dir": dir.to_string_lossy()}),
    )
    .await;
    assert_degraded(&saved, "download /js");
    assert_eq!(
        saved["meta"]["degraded_from"], "js-render",
        "必须写明降级来源：{saved}"
    );
    let path = saved["data"]["saved_to"].as_str().expect("saved_to");
    assert!(
        std::path::Path::new(path).is_file(),
        "降级不代表没下载成功：文件应已落盘（{path}）"
    );
    assert_actionable_channels(&saved, "download /js");
}

/// json_query 在空壳页上**自己的解析失败**不得被伪装成降级（只继承 OK 结果）。
#[tokio::test]
async fn json_query_keeps_parse_failure_instead_of_inheriting() {
    let server =
        http_fixture::Server::start_with(vec![("/js", http_fixture::Response::html(JS_SHELL))])
            .await;
    let mcp = WebctlMcp::new(in_process_context("json-query-shell"));
    let queried = invoke(
        &mcp,
        "json_query",
        json!({"url": server.url("/js"), "expression": ".a"}),
    )
    .await;
    assert_eq!(
        queried["ok"], false,
        "非 JSON 输入必须保持失败语义：{queried}"
    );
    assert_ne!(
        code_of(&queried),
        "DEGRADED",
        "本工具自己的失败不能被伪装成降级：{queried}"
    );
    assert!(
        !queried["error"].as_str().unwrap_or_default().is_empty(),
        "失败必须带说明：{queried}"
    );
}
