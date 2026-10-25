//! 0.2 M1：`resolve_site` 的 MCP 面接线测试。
//!
//! 单独成文件：`tests/mcp.rs` 已接近 1000 行上限（仓库规则），新用例按主题分文件。

mod common;

use common::*;
use serde_json::{Value, json};
use webctl_mcp::WebctlMcp;

/// 进程内调用一次 MCP 工具（未知工具名直接 panic）。
async fn invoke(mcp: &WebctlMcp, name: &str, args: Value) -> Value {
    mcp.invoke(name, args)
        .await
        .unwrap_or_else(|| panic!("工具 {name} 未注册"))
}

/// 离线上下文：resolve_site 的 L3 会联网，**测试必须显式 offline**，否则会打真实外网。
fn offline_ctx(tag: &str) -> webctl_mcp::McpContext {
    webctl_mcp::McpContext::unbound(webctl_core::Config {
        state_dir: temp_dir(tag),
        offline: true,
        ..webctl_core::Config::default()
    })
}
/// 0.2 M1：`resolve_site` 必须在工具面上，并且真的接到 L1 层（内置映射命中）。
#[tokio::test]
async fn resolve_site_builtin_hit_is_wired() {
    let mcp = WebctlMcp::new(in_process_context("resolve-site"));
    let envelope = invoke(&mcp, "resolve_site", json!({"site": "github"})).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["code"], "OK");
    assert_eq!(envelope["data"]["confidence"], "high");
    // 不写完整 URL 字面量：offline lane 会扫描 /tests/ 下的真实外站 URL（只放行回环与保留域名）。
    assert!(
        envelope["data"]["candidates"][0]["url"]
            .as_str()
            .is_some_and(|url| url.ends_with("github.com")),
        "内置映射应给出 github.com 的候选：{envelope}"
    );
    assert_eq!(envelope["data"]["candidates"][0]["channel"], "builtin");
    assert!(
        envelope["meta"]["actionable"].is_null(),
        "可靠定位不应带 actionable：{envelope}"
    );
}

/// 0.2 M1：低置信必须显式降级，`next_steps` 只能提真实工具（用派发表反查）。
#[tokio::test]
async fn resolve_site_low_confidence_degrades_with_real_next_steps() {
    let mcp = WebctlMcp::new(offline_ctx("resolve-site-low"));
    let envelope = invoke(&mcp, "resolve_site", json!({"site": "example"})).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["code"], "DEGRADED");
    assert_eq!(envelope["meta"]["degraded_from"], "resolve-site");
    assert_eq!(
        envelope["data"]["layers"],
        json!(["l1"]),
        "offline 不得尝试 L3"
    );
    let actionable = &envelope["meta"]["actionable"];
    assert_eq!(
        actionable["reason"], "resolve-site-offline",
        "离线时要如实说是 offline 卡住的：{envelope}"
    );
    for step in actionable["next_steps"].as_array().expect("next_steps") {
        let name = step.as_str().expect("channel name");
        assert!(
            webctl_mcp::dispatch::is_known_tool(name),
            "建议的通道 {name} 必须真实存在"
        );
    }
}

/// 0.2 M1：schema 里的参数必须等于实现真正读的（不得出现未接线的字段）。
#[test]
fn resolve_site_schema_declares_only_wired_parameters() {
    let mut server = Server::start(&temp_dir("resolve-schema"), None);
    server.initialize();
    let response = server.request(2, "tools/list", json!({}));
    let tools = response["result"]["tools"].as_array().expect("tools");
    let tool = tools
        .iter()
        .find(|tool| tool["name"] == "resolve_site")
        .expect("resolve_site 必须在 tools/list 里");
    let mut props: Vec<&str> = tool["inputSchema"]["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .map(String::as_str)
        .collect();
    props.sort_unstable();
    assert_eq!(props, vec!["allow_serp_fallback", "max_candidates", "site"]);
}
