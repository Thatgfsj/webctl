//! `extract.fields`（议题 #193）在 MCP 工具面的验收。
//!
//! 覆盖：schema 声明=实现 / 不带 fields 的向后兼容 / fields-only / selector+fields 合并 /
//! 两个都不给 / 空 schema / 全部非法 / 非 HTML 路由 / 截断降级（含合并时的优先级）。
//!
//! 全部用例走进程内 `invoke`（零子进程）：输入要么是 `html` 参数，要么是
//! 127.0.0.1 fixture，不访问外网。

mod common;

use common::*;
use serde_json::{Value, json};
use webctl_mcp::{TOOL_NAMES, WebctlMcp, tool_definitions};

const PAGE: &str = include_str!("../../webctl-web/tests/fixtures/extract/page.html");

/// h1 + 列表：同时验证「合并」与「零命中字段」。
const COMBINED: &str = "<html><body><h1>Title</h1>\
     <ul><li class=\"item\">ONE</li><li class=\"item\">TWO</li></ul></body></html>";

fn server(tag: &str) -> WebctlMcp {
    WebctlMcp::new(in_process_context(tag))
}

async fn invoke(mcp: &WebctlMcp, name: &str, args: Value) -> Value {
    mcp.invoke(name, args)
        .await
        .unwrap_or_else(|| panic!("工具 {name} 未注册"))
}

/// FIELD-MCP-01：工具面仍是 18 个；`extract` 的 inputSchema 只声明已接线的参数。
#[test]
fn fields_schema_declares_only_wired_parameters() {
    let definitions = tool_definitions();
    assert_eq!(definitions.len(), TOOL_NAMES.len(), "工具面仍是 18 个");
    let extract = definitions
        .iter()
        .find(|tool| tool.name == "extract")
        .expect("extract 工具必须在注册表里");
    let schema = serde_json::to_value(extract).expect("tool schema 可序列化");
    let schema_text = schema.to_string();
    let properties = schema["inputSchema"]["properties"]
        .as_object()
        .expect("extract 的 inputSchema.properties");
    let mut declared: Vec<&str> = properties.keys().map(String::as_str).collect();
    declared.sort_unstable();
    assert_eq!(
        declared,
        [
            "char_budget",
            "context_chars",
            "fields",
            "html",
            "max_results",
            "selector",
            "start_index",
            "url",
        ],
        "声明的参数必须与 dispatch 实际读的参数一一对应"
    );
    // selector 从必填变为可选（与 fields 至少给一个由运行时校验），旧客户端仍可直接传。
    let required = schema["inputSchema"]["required"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !required.iter().any(|value| value == "selector"),
        "selector 不应再是 required：{required:?}"
    );
    // fields 是「字段名 -> {selector, attr, all}」的对象；可选参数因此是 ["object","null"]。
    let kind = properties["fields"]["type"].clone();
    let is_object = kind == json!("object")
        || kind
            .as_array()
            .is_some_and(|kinds| kinds.contains(&json!("object")));
    assert!(is_object, "fields 必须是对象（可为 null）：{kind}");
    for key in ["\"selector\"", "\"attr\"", "\"all\""] {
        assert!(
            schema_text.contains(key),
            "fields 子 schema 缺少 {key}: {schema_text}"
        );
    }
}

/// FIELD-MCP-02：只给 selector 时输出与引入 fields 之前完全一致（零回归）。
#[tokio::test]
async fn fields_selector_only_output_is_unchanged() {
    let mcp = server("fields-compat");
    let envelope = invoke(
        &mcp,
        "extract",
        json!({"html": PAGE, "selector": "li.item"}),
    )
    .await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["code"], "OK", "{envelope}");
    assert_eq!(envelope["meta"]["degraded_from"], Value::Null, "{envelope}");
    let data = envelope["data"].as_object().expect("data");
    for key in [
        "selector",
        "matches",
        "observed_matches",
        "more_matches",
        "truncated",
        "truncated_reasons",
        "conclusion",
        "stability",
    ] {
        assert!(
            data.contains_key(key),
            "既有字段 {key} 不得消失：{envelope}"
        );
    }
    assert!(
        !data.contains_key("fields"),
        "不带 fields 时不得凭空多出字段：{envelope}"
    );
    assert!(!data.contains_key("diagnostics"), "{envelope}");
    let matches = data["matches"].as_array().expect("matches");
    assert_eq!(matches.len(), 3, "{envelope}");
    assert_eq!(matches[0]["text"], "ITEM-ONE");
}

/// FIELD-MCP-03：只给 fields 时按 schema 取值，不返回选择器路径的字段。
#[tokio::test]
async fn fields_only_returns_schema_values() {
    let mcp = server("fields-only");
    let envelope = invoke(
        &mcp,
        "extract",
        json!({"html": COMBINED, "fields": {"title": {"selector": "h1"},
            "items": {"selector": "li.item", "all": true}}}),
    )
    .await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["code"], "OK", "{envelope}");
    assert_eq!(envelope["meta"]["trust"], "untrusted_page_content");
    let data = envelope["data"].as_object().expect("data");
    assert_eq!(data["fields"]["title"], "Title", "{envelope}");
    assert_eq!(data["fields"]["items"], json!(["ONE", "TWO"]), "{envelope}");
    assert_eq!(data["diagnostics"], json!([]), "{envelope}");
    assert!(!data.contains_key("matches"), "{envelope}");
}

/// FIELD-MCP-04：selector 与 fields 同时给出时合并：matches 不变 + fields/diagnostics 并入。
#[tokio::test]
async fn fields_and_selector_merge() {
    let mcp = server("fields-merge");
    let envelope = invoke(
        &mcp,
        "extract",
        json!({"html": COMBINED, "selector": "li.item",
            "fields": {"title": {"selector": "h1"}, "missing": {"selector": "h2"}}}),
    )
    .await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    let data = envelope["data"].as_object().expect("data");
    assert_eq!(
        data["matches"].as_array().expect("matches").len(),
        2,
        "{envelope}"
    );
    assert_eq!(data["fields"]["title"], "Title", "{envelope}");
    assert_eq!(data["fields"]["missing"], Value::Null, "{envelope}");
    let diagnostics = data["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(diagnostics.len(), 1, "{envelope}");
    assert_eq!(diagnostics[0]["field"], "missing", "{envelope}");
    assert_eq!(diagnostics[0]["kind"], "zero-matches", "{envelope}");
}

/// FIELD-MCP-05：selector 与 fields 都不给 -> PARSE（且不发起取回）。
#[tokio::test]
async fn fields_missing_target_is_parse_error() {
    let mcp = server("fields-none");
    let envelope = invoke(&mcp, "extract", json!({"html": "<p>x</p>"})).await;
    assert_eq!(envelope["ok"], false, "{envelope}");
    assert_eq!(envelope["code"], "PARSE", "{envelope}");
    assert!(
        envelope["error"]
            .as_str()
            .is_some_and(|text| text.contains("至少给一个")),
        "{envelope}"
    );
    // url 分支同理：必须在取回之前拦下（否则会得到 NETWORK 而不是 PARSE）。
    let unreachable = invoke(&mcp, "extract", json!({"url": "http://127.0.0.1:1/never"})).await;
    assert_eq!(unreachable["code"], "PARSE", "{unreachable}");
}

/// FIELD-MCP-06：空 fields 对象 / 全部字段非法 -> PARSE（不是成功但全 null）。
#[tokio::test]
async fn fields_declaration_errors_are_parse() {
    let mcp = server("fields-decl");
    let empty = invoke(&mcp, "extract", json!({"html": PAGE, "fields": {}})).await;
    assert_eq!(empty["ok"], false, "{empty}");
    assert_eq!(empty["code"], "PARSE", "{empty}");
    assert!(empty["data"].is_null(), "{empty}");

    let invalid = invoke(
        &mcp,
        "extract",
        json!({"html": PAGE, "fields": {"a": {"selector": "div["}}}),
    )
    .await;
    assert_eq!(invalid["ok"], false, "{invalid}");
    assert_eq!(invalid["code"], "PARSE", "{invalid}");
}

/// FIELD-MCP-07：非 HTML 内容 -> UNSUPPORTED 降级（fields 与 selector 同语义）。
#[tokio::test]
async fn fields_on_non_html_url_is_unsupported() {
    let fixture = http_fixture::Server::start_with(vec![(
        "/doc",
        http_fixture::Response::pdf(b"%PDF-1.4".to_vec()),
    )])
    .await;
    let mcp = server("fields-pdf");
    let envelope = invoke(
        &mcp,
        "extract",
        json!({"url": fixture.url("/doc"), "fields": {"title": {"selector": "h1"}}}),
    )
    .await;
    assert_eq!(envelope["ok"], true, "非 HTML 是降级不是失败：{envelope}");
    assert_eq!(envelope["code"], "UNSUPPORTED", "{envelope}");
    assert_eq!(
        envelope["meta"]["degraded_from"], "content-type:pdf",
        "{envelope}"
    );
    assert_eq!(envelope["data"]["fields"], json!({}), "{envelope}");
}

/// FIELD-MCP-08：`all=true` 被 max_results 截断 -> LIMIT + fields-truncated。
#[tokio::test]
async fn fields_truncation_is_limit() {
    let mcp = server("fields-limit");
    let envelope = invoke(
        &mcp,
        "extract",
        json!({"html": PAGE, "max_results": 2,
            "fields": {"items": {"selector": "li.item", "all": true}}}),
    )
    .await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["code"], "LIMIT", "{envelope}");
    assert_eq!(
        envelope["meta"]["degraded_from"], "fields-truncated",
        "{envelope}"
    );
    assert_eq!(
        envelope["data"]["fields"]["items"],
        json!(["ITEM-ONE", "ITEM-TWO"]),
        "{envelope}"
    );
    assert_eq!(
        envelope["data"]["diagnostics"][0]["kind"], "truncated",
        "{envelope}"
    );
}

/// FIELD-MCP-09：合并时两侧都截断，保留 selector 自己的来源码（fields 的截断留在诊断里）。
#[tokio::test]
async fn fields_merge_keeps_selector_degradation_reason() {
    let mcp = server("fields-limit-merge");
    let envelope = invoke(
        &mcp,
        "extract",
        json!({"html": PAGE, "selector": "li.item", "max_results": 2,
            "fields": {"items": {"selector": "li.item", "all": true}}}),
    )
    .await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["code"], "LIMIT", "{envelope}");
    assert_eq!(
        envelope["meta"]["degraded_from"], "max-results",
        "{envelope}"
    );
    assert_eq!(
        envelope["data"]["matches"]
            .as_array()
            .expect("matches")
            .len(),
        2,
        "{envelope}"
    );
    assert_eq!(
        envelope["data"]["fields"]["items"],
        json!(["ITEM-ONE", "ITEM-TWO"]),
        "{envelope}"
    );
}
