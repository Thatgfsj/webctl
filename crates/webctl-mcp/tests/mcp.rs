mod common;

use common::*;
use common::{http_fixture, resources};
use std::time::Duration;

use serde_json::{Value, json};
use webctl_core::Config;
use webctl_mcp::{McpContext, TOOL_NAMES, WebctlMcp, qualified_name};

/// MCP-01：initialize 协商 protocolVersion。
#[test]
fn mcp_01_initialize_negotiates_protocol_version() {
    let mut server = Server::start(&temp_dir("init"), None);
    let response = server.initialize();
    let version = response["result"]["protocolVersion"]
        .as_str()
        .expect("protocolVersion");
    assert_eq!(version, "2025-06-18", "应回显客户端请求且服务器支持的版本");
    assert_eq!(response["result"]["serverInfo"]["name"], "webctl");
}

/// MCP-02：tools/list 恰好 TOOL_NAMES 那么多个且名字与 PLAN §3.7 完全一致。
#[test]
fn mcp_02_tools_list_is_exactly_the_plan_set() {
    let mut server = Server::start(&temp_dir("list"), None);
    server.initialize();
    let response = server.request(2, "tools/list", json!({}));
    let tools = response["result"]["tools"].as_array().expect("tools array");
    assert_eq!(
        tools.len(),
        TOOL_NAMES.len(),
        "tools/list 数量必须与 TOOL_NAMES 一致"
    );
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect();
    assert_eq!(
        names,
        TOOL_NAMES.to_vec(),
        "工具名与顺序必须与 PLAN §3.7 一致"
    );
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), TOOL_NAMES.len(), "工具名不得重复");
}

/// MCP-03：每个 inputSchema 合法（对象 + properties 对象 + required 已声明）。
#[test]
fn mcp_03_input_schemas_are_valid_json_schema() {
    let mut server = Server::start(&temp_dir("schema"), None);
    server.initialize();
    let response = server.request(2, "tools/list", json!({}));
    for tool in response["result"]["tools"].as_array().expect("tools") {
        let name = tool["name"].as_str().expect("name");
        let schema = &tool["inputSchema"];
        assert!(schema.is_object(), "{name}: inputSchema 必须是 JSON 对象");
        assert_eq!(schema["type"], "object", "{name}: 顶层 type 必须为 object");
        assert!(
            schema["properties"].is_object(),
            "{name}: properties 必须是对象"
        );
        if let Some(required) = schema.get("required") {
            assert!(required.is_array(), "{name}: required 必须是数组");
            for entry in required.as_array().expect("required") {
                let field = entry.as_str().expect("required entry");
                assert!(
                    schema["properties"].get(field).is_some(),
                    "{name}: required 字段 {field} 必须在 properties 中声明"
                );
            }
        }
    }
}

/// MCP-04：工具名满足 DSH 的 mcp__webctl__<rawName> 命名契约。
#[test]
fn mcp_04_qualified_names_follow_the_function_name_contract() {
    for name in TOOL_NAMES {
        assert!(!name.is_empty() && name.len() <= 32);
        assert!(
            name.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'),
            "{name} 只允许小写字母与下划线"
        );
        assert_eq!(qualified_name(name), format!("mcp__webctl__{name}"));
    }
    assert!(TOOL_NAMES.iter().all(|name| !name.contains("mcp__")));
}

/// MCP-05：服务器指令 ≤ 32768 字节。
#[test]
fn mcp_05_instructions_stay_under_32k() {
    let mut server = Server::start(&temp_dir("instructions"), None);
    let response = server.initialize();
    let instructions = response["result"]["instructions"]
        .as_str()
        .expect("instructions");
    assert!(
        instructions.len() <= 32 * 1024,
        "指令 {} 字节",
        instructions.len()
    );
    assert!(!instructions.is_empty());
}

/// MCP-06：服务可被启动两次而不冲突（stdio 探测场景：无独占锁/端口占用）。
#[test]
fn mcp_06_two_servers_start_without_conflict() {
    let dir = temp_dir("twice");
    let mut first = Server::start(&dir, None);
    let mut second = Server::start(&dir, None);
    let one = first.initialize();
    let two = second.initialize();
    assert_eq!(one["result"]["serverInfo"]["name"], "webctl");
    assert_eq!(two["result"]["serverInfo"]["name"], "webctl");
    let list_a = first.request(2, "tools/list", json!({}));
    let list_b = second.request(3, "tools/list", json!({}));
    assert_eq!(
        list_a["result"]["tools"].as_array().expect("a").len(),
        TOOL_NAMES.len()
    );
    assert_eq!(
        list_b["result"]["tools"].as_array().expect("b").len(),
        TOOL_NAMES.len()
    );
}

/// MCP-07：跨 Session 调用被拒且错误可读。
#[test]
fn mcp_07_cross_session_calls_are_rejected() {
    let mut server = Server::start(&temp_dir("session"), Some("owner-alpha"));
    server.initialize();
    let rejected = server.call(2, "session", json!({"action": "list", "session": "beta"}));
    assert!(
        rejected.get("error").is_none(),
        "必须是结果信封，不是协议错误：{rejected}"
    );
    let envelope = envelope_of(&rejected);
    assert_eq!(envelope["ok"], false);
    let message = envelope["error"].as_str().expect("readable error");
    assert!(
        message.contains("owner-alpha") && message.contains("beta"),
        "{message}"
    );
    assert!(message.contains("归属拒绝"), "{message}");

    let allowed = server.call(
        3,
        "session",
        json!({"action": "list", "session": "owner-alpha"}),
    );
    assert_eq!(envelope_of(&allowed)["ok"], true);
}

/// MCP-08：配置/指令中不写 toolOrder，也不逐个枚举 webctl 工具名。
#[test]
fn mcp_08_no_tool_order_in_config_or_instructions() {
    let mut server = Server::start(&temp_dir("toolorder"), None);
    let response = server.initialize();
    let instructions = response["result"]["instructions"]
        .as_str()
        .expect("instructions");
    assert!(
        !instructions.contains("toolOrder"),
        "指令不得出现 toolOrder"
    );
    let listed = server.request(2, "tools/list", json!({}));
    let serialized = serde_json::to_string(&listed).expect("serialize");
    assert!(
        !serialized.contains("toolOrder"),
        "tools/list 不得携带 toolOrder"
    );
    let snippet = webctl_mcp::instructions::dsh_config_snippet();
    assert!(!snippet.contains("toolOrder"));
    for name in TOOL_NAMES {
        assert!(!snippet.contains(name), "配置片段不得枚举工具名 {name}");
    }
}

/// MCP-09（L1）：单次调用预算生效，超时以 TIMEOUT 信封返回。
#[test]
fn mcp_09_call_budget_times_out_as_envelope() {
    let context = in_process_context("budget");
    let server = WebctlMcp::new(context).with_call_budget(Duration::from_millis(1));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let value = runtime.block_on(server.guard(async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        json!({"ok": true})
    }));
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "TIMEOUT");
    assert!(value["error"].as_str().expect("error").contains("预算"));
    let default_server = WebctlMcp::new(in_process_context("budget2"));
    assert_eq!(default_server.call_budget().as_secs(), 60);
}

/// MCP-10：已知工具的执行失败以结果信封返回，而不是 JSON-RPC 协议错误。
#[test]
fn mcp_10_tool_failures_use_the_envelope() {
    let mut server = Server::start(&temp_dir("envelope"), None);
    server.initialize();
    let response = server.call(2, "extract", json!({"selector": "h1"}));
    assert!(
        response.get("error").is_none(),
        "失败不得是协议错误：{response}"
    );
    let envelope = envelope_of(&response);
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["code"], "BAD_URL");
    assert!(envelope["data"].is_null());
    assert!(!envelope["error"].as_str().expect("error").is_empty());

    // 未注册的工具名才是协议错误（不可路由）。
    let unknown = server.call(3, "definitely_not_a_tool", json!({}));
    assert_eq!(unknown["error"]["code"], -32601);
}

/// MCP-11：大结果落盘返回路径，不无脑内联。
#[test]
fn mcp_11_large_results_are_spilled_to_disk() {
    let mut server = Server::start(&temp_dir("spill"), None);
    server.initialize();
    let big = "y".repeat(200_000);
    let html = format!("<html><body><p>{big}</p></body></html>");
    let response = server.call(
        2,
        "extract",
        json!({"selector": "p", "html": html, "char_budget": 1_000_000}),
    );
    let envelope = envelope_of(&response);
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["data"]["inline"], false, "大结果必须落盘");
    let saved = envelope["data"]["saved_to"].as_str().expect("saved_to");
    let written = std::fs::read_to_string(saved)
        .unwrap_or_else(|error| panic!("落盘文件不可读 {saved}: {error}"));
    assert!(written.contains("\"selector\""), "落盘文件应是完整信封");
    assert!(written.len() > 32 * 1024);
}

/// MCP-12：SKILL.md 声明的工具与 tools/list 一致（脚本同源校验）。
#[test]
fn mcp_12_skill_md_matches_tools_list() {
    let root = resources::workspace_root();
    let skill = std::fs::read_to_string(root.join("SKILL.md")).expect("读 SKILL.md");
    let mut declared: Vec<&str> = Vec::new();
    let mut inside = false;
    for line in skill.lines() {
        if line.contains("tools:start") {
            inside = true;
            continue;
        }
        if line.contains("tools:end") {
            inside = false;
            continue;
        }
        if !inside {
            continue;
        }
        let Some(rest) = line.strip_prefix("- `") else {
            continue;
        };
        let Some(end) = rest.find("`") else {
            continue;
        };
        declared.push(&rest[..end]);
    }
    assert_eq!(
        declared.len(),
        TOOL_NAMES.len(),
        "SKILL.md 声明的工具数必须与 TOOL_NAMES 一致：{declared:?}"
    );
    assert_eq!(
        declared,
        TOOL_NAMES.to_vec(),
        "SKILL.md 与 tools/list 必须一致"
    );
}

/// MCP-13 / STATE-06：MCP 与 CLI 共享 state dir（子进程经 core 解析 WEBCTL_STATE_DIR）。
#[test]
fn mcp_13_shares_state_dir_with_core_resolution() {
    let dir = temp_dir("state");
    let mut server = Server::start(&dir, None);
    server.initialize();
    let response = server.call(2, "session", json!({"action": "save", "name": "shared"}));
    let envelope = envelope_of(&response);
    assert_eq!(envelope["ok"], true, "{envelope}");
    let expected = dir.join("sessions").join("shared.json");
    assert!(
        expected.is_file(),
        "MCP 会话必须落在 core 解析的 state dir：{}",
        expected.display()
    );
}

/// SEC-09：MCP 路径返回的页面内容一律 untrusted_page_content。
#[test]
fn sec_09_mcp_content_is_untrusted() {
    let mut server = Server::start(&temp_dir("trust"), None);
    server.initialize();
    let response = server.call(
        2,
        "extract",
        json!({"selector": "h1", "html": "<h1>hi</h1>"}),
    );
    let envelope = envelope_of(&response);
    assert_eq!(envelope["meta"]["trust"], "untrusted_page_content");

    let caps = envelope_of(&server.call(3, "caps", json!({})));
    assert_eq!(
        caps["meta"]["trust"], "trusted_local",
        "caps 是本地自检数据"
    );
}

/// ENV-06：同一异常在 CLI 与 MCP 两条路径得到同一 code（共享取回层）。
#[test]
fn env_06_cli_and_mcp_map_the_same_error_code() {
    let mut server = Server::start(&temp_dir("env06"), None);
    server.initialize();
    let mcp = envelope_of(&server.call(2, "fetch", json!({"url": "notaurl"})));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let cli = runtime.block_on(async {
        webctl_web::transport::fetch("notaurl", &webctl_web::transport::FetchOptions::default())
            .await
    });
    assert_eq!(mcp["code"], "BAD_URL");
    assert_eq!(mcp["code"], serde_json::to_value(cli.code).expect("code"));
}

/// 派发表与 caps 一致：is_known_tool 对 TOOL_NAMES 里每个名字都为真。
#[test]
fn all_tools_are_dispatchable() {
    for name in TOOL_NAMES {
        assert!(webctl_mcp::dispatch::is_known_tool(name), "{name} 未注册");
    }
    assert!(!webctl_mcp::dispatch::is_known_tool("screenshot"));
}

/// embedded_data（0.1.2 / 议题 #114）：从内联 HTML 抽 JSON-LD，**零 JS 执行**。
#[tokio::test]
async fn embedded_data_extracts_json_ld_from_inline_html() {
    let mcp = WebctlMcp::new(in_process_context("embedded-ok"));
    let html = r#"<html><head>
      <script type="application/ld+json">{"@type":"Article","headline":"内嵌标题"}</script>
      </head><body><div id="root"></div></body></html>"#;
    let envelope = invoke(&mcp, "embedded_data", json!({ "html": html })).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(code_of(&envelope), "OK");
    let entries = envelope["data"]["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 1, "{envelope}");
    assert_eq!(entries[0]["source"], "ld+json");
    assert_eq!(entries[0]["value"]["headline"], "内嵌标题");
}

/// embedded_data：没有内嵌数据是**合法空结果**（ok=true + OK），不是解析失败。
#[tokio::test]
async fn embedded_data_empty_result_is_not_a_parse_failure() {
    let mcp = WebctlMcp::new(in_process_context("embedded-empty"));
    let html = r#"<html><body><p>纯静态</p><script src="/app.js"></script></body></html>"#;
    let envelope = invoke(&mcp, "embedded_data", json!({ "html": html })).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(code_of(&envelope), "OK");
    assert!(
        envelope["data"]["entries"]
            .as_array()
            .expect("entries")
            .is_empty()
    );
    assert_eq!(envelope["data"]["scanned_scripts"], 1);
}

/// embedded_data：容器在但 JSON 坏了 —— 必须是 PARSE（由 web crate 的信封语义保证）。
#[tokio::test]
async fn embedded_data_broken_payload_is_parse_error() {
    let mcp = WebctlMcp::new(in_process_context("embedded-parse"));
    let html = r#"<script type="application/ld+json">{"a": }</script>"#;
    let envelope = invoke(&mcp, "embedded_data", json!({ "html": html })).await;
    assert_eq!(envelope["ok"], false, "{envelope}");
    assert_eq!(code_of(&envelope), "PARSE");
}

/// embedded_data 的 inputSchema **只声明实现真正支持的参数**。
///
/// 起因：第一版声明了 `path` 但实现没接线（会静默忽略），复审据此驳回。
/// 这条用例把"声明 = 实现"钉成可机械校验的约束。
#[test]
fn embedded_data_schema_declares_only_wired_parameters() {
    let mut server = Server::start(&temp_dir("embedded-schema"), None);
    server.initialize();
    let response = server.request(2, "tools/list", json!({}));
    let tools = response["result"]["tools"].as_array().expect("tools");
    let tool = tools
        .iter()
        .find(|tool| tool["name"] == "embedded_data")
        .expect("embedded_data 必须在 tools/list 里");
    let mut props: Vec<&str> = tool["inputSchema"]["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .map(String::as_str)
        .collect();
    props.sort_unstable();
    assert_eq!(
        props,
        vec![
            "html",
            "include_inline_state",
            "max_entries",
            "max_entry_bytes",
            "url"
        ],
        "schema 里的参数必须都是实现真正读的；不得出现未接线的 path"
    );
}

/// embedded_data：`url` 与 `html` 同时给出必须报错，不能静默挑一个。
#[tokio::test]
async fn embedded_data_rejects_url_and_html_together() {
    let mcp = WebctlMcp::new(in_process_context("embedded-both"));
    let envelope = invoke(
        &mcp,
        "embedded_data",
        json!({ "url": "https://example.test/", "html": "<p>x</p>" }),
    )
    .await;
    assert_eq!(envelope["ok"], false, "{envelope}");
    assert_eq!(code_of(&envelope), "BAD_URL");
}

/// #115：SPA 支持面的通道必须是**真实注册的工具**（候选表写错即红）。
#[test]
fn spa_surface_channels_are_registered_tools() {
    for name in webctl_mcp::caps::SUGGESTED_CHANNELS {
        assert!(
            webctl_mcp::dispatch::is_known_tool(name),
            "建议的通道 {name} 必须是真实注册的工具"
        );
    }
    let surface = webctl_mcp::caps::spa_surface();
    assert!(!surface.channels.is_empty(), "SPA 通道不得为空");
    assert_eq!(surface.detected_containers.len(), 4, "应报告 4 类容器");
    assert!(
        surface
            .channels
            .iter()
            .all(|name| webctl_mcp::dispatch::is_known_tool(name)),
        "过滤后仍必须全是真实工具：{:?}",
        surface.channels
    );
}

/// #115：降级信封要带"为什么 + 还能试什么"，且 `next_steps` 必须是真实工具。
#[tokio::test]
async fn degraded_envelope_carries_actionable_next_steps() {
    let server =
        http_fixture::Server::start_with(vec![("/js", http_fixture::Response::html(JS_SHELL))])
            .await;
    let mcp = WebctlMcp::new(in_process_context("actionable"));
    let extracted = invoke(
        &mcp,
        "extract",
        json!({"url": server.url("/js"), "selector": "div.quote"}),
    )
    .await;
    assert_degraded(&extracted, "extract /js");
    let actionable = &extracted["meta"]["actionable"];
    assert!(actionable.is_object(), "降级必须带可行动信息：{extracted}");
    assert!(
        actionable["reason"].as_str().is_some_and(|s| !s.is_empty()),
        "要有机器可读原因：{actionable}"
    );
    assert!(
        !actionable["detected"]
            .as_array()
            .expect("detected")
            .is_empty(),
        "要带上观察到的证据：{actionable}"
    );
    for step in actionable["next_steps"].as_array().expect("next_steps") {
        let name = step.as_str().expect("channel name");
        assert!(
            webctl_mcp::dispatch::is_known_tool(name),
            "建议的通道 {name} 必须真实存在（用派发表反查）"
        );
    }
}

/// #115：未降级的成功信封**不得**带 actionable（协议误用会被 validate 拦下）。
#[tokio::test]
async fn ok_envelope_has_no_actionable() {
    let mcp = WebctlMcp::new(in_process_context("no-actionable"));
    let envelope = invoke(&mcp, "embedded_data", json!({"html": "<p>静态</p>"})).await;
    assert_eq!(envelope["ok"], true);
    assert!(envelope["meta"]["actionable"].is_null(), "{envelope}");
}

/// #115 的口径边界：**只有** JS 渲染降级带 `meta.actionable`。
///
/// 这条用例把"当前事实"钉住，防止两种偏差：① 文档说得比实现多（复审在 #123 抓到过）；
/// ② 悄悄改了行为。等 [#124](https://github.com/Thatgfsj/webctl/issues/124) 把其余降级
/// 路径接上后，本用例应当改成断言"该路径也带 actionable"。
#[tokio::test]
async fn limit_degradation_has_no_actionable_yet() {
    let server = http_fixture::Server::start_with(vec![(
        "/static",
        http_fixture::Response::html(STATIC_QUOTES),
    )])
    .await;
    let mcp = WebctlMcp::new(in_process_context("limit-no-actionable"));
    let limited = invoke(
        &mcp,
        "extract",
        json!({"url": server.url("/static"), "selector": "div.quote", "max_results": 1}),
    )
    .await;
    assert_eq!(code_of(&limited), "LIMIT", "{limited}");
    assert!(
        limited["meta"]["actionable"].is_null(),
        "LIMIT 路径目前不带 actionable（见 #124）；若要改，先改这条用例与文档：{limited}"
    );
}

/// #115 复审修正：**传播路径不得丢掉 actionable**。
///
/// 复审实测：`paginate` 在 JS 空壳页上 `degraded_from=js-render` 但 `actionable=null`——
/// `session/engine.rs` 重建信封时把上游 guard 挂的信息丢了。这条用例钉住修复。
#[tokio::test]
async fn paginate_propagates_actionable_from_upstream() {
    let server =
        http_fixture::Server::start_with(vec![("/js", http_fixture::Response::html(JS_SHELL))])
            .await;
    let mcp = WebctlMcp::new(in_process_context("paginate-actionable"));
    let paged = invoke(
        &mcp,
        "paginate",
        json!({"url": server.url("/js"), "max_pages": 2}),
    )
    .await;
    assert_degraded(&paged, "paginate /js");
    // 夹具只服务 /js，翻到第 2 页必然 404 —— 所以这里的 degraded_from 是
    // `pagination-fetch-failed`（而不是 js-render）。恰恰要证明：**后续页失败也不丢可行动信息**。
    assert!(
        paged["meta"]["degraded_from"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "降级来源必须写明：{paged}"
    );
    let actionable = &paged["meta"]["actionable"];
    assert!(
        actionable.is_object(),
        "paginate 必须透传上游的 actionable，不能只给 degraded_from：{paged}"
    );
    assert!(
        actionable["reason"].as_str().is_some_and(|s| !s.is_empty()),
        "原因码不得为空：{actionable}"
    );
    for step in actionable["next_steps"].as_array().expect("next_steps") {
        let name = step.as_str().expect("channel");
        assert!(
            webctl_mcp::dispatch::is_known_tool(name),
            "通道 {name} 必须是真实工具"
        );
    }
}

/// #119 复审补的覆盖缺口：`paginate` **正常收尾**分支（不触发翻页失败）也要带 actionable。
///
/// #123 第 3 轮复审发现：原用例走的是 `pagination-fetch-failed` 早返回，
/// 正常收尾分支（真站 max_pages=1 那条）没有测试钉住（变异 M3 不会变红）。
#[tokio::test]
async fn paginate_normal_finish_keeps_actionable() {
    let server =
        http_fixture::Server::start_with(vec![("/js", http_fixture::Response::html(JS_SHELL))])
            .await;
    let mcp = WebctlMcp::new(in_process_context("paginate-finish"));
    // max_pages=1：只取第一页，不会去翻不存在的第 2 页 → 走正常收尾分支。
    let paged = invoke(
        &mcp,
        "paginate",
        json!({"url": server.url("/js"), "max_pages": 1}),
    )
    .await;
    assert_degraded(&paged, "paginate /js max_pages=1");
    assert!(
        paged["meta"]["degraded_from"]
            .as_str()
            .expect("from")
            .contains("js-render"),
        "正常收尾也应记录 js-render：{paged}"
    );
    assert!(
        paged["meta"]["actionable"].is_object(),
        "正常收尾分支同样不得丢 actionable：{paged}"
    );
}

/// #128 步骤 2：**默认关闭**时，内容只在脚本里的页面仍走既有降级路径（零回归）。
#[tokio::test]
async fn js_render_off_keeps_existing_degradation() {
    let server =
        http_fixture::Server::start_with(vec![("/js", http_fixture::Response::html(JS_SHELL))])
            .await;
    let mcp = WebctlMcp::new(in_process_context("js-off"));
    let extracted = invoke(
        &mcp,
        "extract",
        json!({"url": server.url("/js"), "selector": "div.quote"}),
    )
    .await;
    assert_degraded(&extracted, "extract /js（开关关闭）");
    assert_eq!(
        extracted["data"]["matches"]
            .as_array()
            .expect("matches")
            .len(),
        0,
        "关闭时不得执行页面脚本：{extracted}"
    );
}

/// #128 步骤 2 的核心验收：**开关开启**后，内容只在 `document.write` 里的页面能抽到内容。
#[tokio::test]
async fn js_render_on_extracts_content_from_script_only_page() {
    let server =
        http_fixture::Server::start_with(vec![("/js", http_fixture::Response::html(JS_SHELL))])
            .await;
    let config = Config {
        state_dir: temp_dir("js-on"),
        js_render: true,
        ..Config::default()
    };
    let mcp = WebctlMcp::new(McpContext::unbound(config));
    let extracted = invoke(
        &mcp,
        "extract",
        json!({"url": server.url("/js"), "selector": "div.quote"}),
    )
    .await;
    assert_eq!(extracted["ok"], true, "{extracted}");
    let matches = extracted["data"]["matches"].as_array().expect("matches");
    assert_eq!(matches.len(), 10, "渲染后应命中 10 条：{extracted}");
}

/// #128 步骤 2：渲染**失败**必须降级（A3），不许伪装成"成功但空"。
#[tokio::test]
async fn js_render_failure_is_degraded_not_silent_success() {
    let loop_page = "<html><body><div class=\"quote\">静态一条</div>\
                     <script>while (true) {}</script></body></html>";
    let server =
        http_fixture::Server::start_with(vec![("/loop", http_fixture::Response::html(loop_page))])
            .await;
    let config = Config {
        state_dir: temp_dir("js-loop"),
        js_render: true,
        ..Config::default()
    };
    let mcp = WebctlMcp::new(McpContext::unbound(config));
    let extracted = invoke(
        &mcp,
        "extract",
        json!({"url": server.url("/loop"), "selector": "div.quote"}),
    )
    .await;
    assert_eq!(
        extracted["ok"], true,
        "原始 HTML 仍可用，所以是降级不是失败：{extracted}"
    );
    assert_eq!(code_of(&extracted), "DEGRADED", "{extracted}");
    assert_eq!(
        extracted["meta"]["degraded_from"], "js-render-failed",
        "{extracted}"
    );
    assert!(
        extracted["meta"]["actionable"]["reason"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "必须给出原因：{extracted}"
    );
    assert!(extracted["error"].as_str().is_some_and(|s| !s.is_empty()));
}

/// #128 步骤 2：`caps` 必须报告**已接线**，且说清覆盖范围（不能只说"支持 JS 渲染"）。
#[test]
fn caps_reports_js_wired_with_coverage() {
    let off = webctl_mcp::caps::js_capability(&Config::default());
    assert!(!off.enabled);
    assert!(off.wired, "本步骤后应报告已接线");
    assert!(
        off.reason.contains("开关开启"),
        "要说清触发条件：{}",
        off.reason
    );
    assert!(
        off.reason.contains("最小 DOM") || off.reason.contains("不是浏览器"),
        "要说清能力边界：{}",
        off.reason
    );
    let on = webctl_mcp::caps::js_capability(&Config {
        js_render: true,
        ..Config::default()
    });
    assert!(on.enabled);
    assert!(on.wired);
}

/// 进程内调用一次 MCP 工具（未知工具名直接 panic）。
async fn invoke(mcp: &WebctlMcp, name: &str, args: Value) -> Value {
    mcp.invoke(name, args)
        .await
        .unwrap_or_else(|| panic!("工具 {name} 未注册"))
}

/// 信封 code 的字符串形式。
fn code_of(envelope: &Value) -> &str {
    envelope["code"].as_str().unwrap_or("<missing>")
}

/// 信封 code 的 JSON 形式（与 [`code_of`] 对齐比较）。
fn code_value(code: webctl_core::ErrorCode) -> Value {
    serde_json::to_value(code).expect("code")
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
        "{context}: 禁止「成功但 0 条」：{envelope}"
    );
    assert!(
        envelope["meta"]["degraded_from"]
            .as_str()
            .is_some_and(|from| !from.is_empty()),
        "{context}: meta.degraded_from 必须非空：{envelope}"
    );
}

/// DEGRADE-* / #82 转正：JS 页在 MCP 路径必须传播降级，静态页零命中仍是 OK 空数组。
///
/// 本用例是原 `mcp_js_degradation_pending_pr09`（#[ignore] panic 哨兵）转正后的真实验收：
/// - fetch：原始 HTML 在 data 里，但 code 非 OK、degraded_from 非空；
/// - extract：零命中不是「成功但 0 条」，而是 DEGRADED/UNSUPPORTED；
/// - paginate：SPA 分页继承库层守卫，不得被吞成 OK；
/// - 静态对照页零命中：ok=true + code=OK + 空数组（EXTRACT-06 不被误伤）。
#[tokio::test]
async fn mcp_js_degradation_propagates() {
    let server = http_fixture::Server::start_with(vec![
        ("/js", http_fixture::Response::html(JS_SHELL)),
        ("/static", http_fixture::Response::html(STATIC_QUOTES)),
    ])
    .await;
    let mcp = WebctlMcp::new(in_process_context("deg-propagate"));

    let fetched = invoke(&mcp, "fetch", json!({"url": server.url("/js")})).await;
    assert_degraded(&fetched, "fetch /js");
    assert_eq!(fetched["meta"]["trust"], "untrusted_page_content");

    let extracted = invoke(
        &mcp,
        "extract",
        json!({"url": server.url("/js"), "selector": "div.quote"}),
    )
    .await;
    assert_degraded(&extracted, "extract /js");
    assert!(
        extracted["data"]["matches"]
            .as_array()
            .expect("matches")
            .is_empty(),
        "不得编造命中：{extracted}"
    );

    let paginated = invoke(
        &mcp,
        "paginate",
        json!({"url": server.url("/js"), "max_pages": 2}),
    )
    .await;
    assert_degraded(&paginated, "paginate /js");

    let static_zero = invoke(
        &mcp,
        "extract",
        json!({"url": server.url("/static"), "selector": "div.never-matches"}),
    )
    .await;
    assert_eq!(static_zero["ok"], true, "{static_zero}");
    assert_eq!(code_of(&static_zero), "OK", "{static_zero}");
    assert!(
        static_zero["meta"]["degraded_from"].is_null(),
        "{static_zero}"
    );
    assert!(
        static_zero["data"]["matches"]
            .as_array()
            .expect("matches")
            .is_empty(),
        "{static_zero}"
    );
}

/// #82：所有返回页面内容的 MCP 路径都遵守 PR 09——JS 页零结果不允许 OK。
#[tokio::test]
async fn mcp_deg_search_and_listing_paths_follow_pr09() {
    let server = http_fixture::Server::start_with(vec![
        ("/js", http_fixture::Response::html(JS_SHELL)),
        ("/static", http_fixture::Response::html(STATIC_QUOTES)),
    ])
    .await;
    let mcp = WebctlMcp::new(in_process_context("deg-paths"));

    for (tool, args) in [
        (
            "search",
            json!({"url": server.url("/js"), "query": "no-such-needle"}),
        ),
        (
            "links",
            json!({"url": server.url("/js"), "domains": ["never.example"]}),
        ),
        ("tables", json!({"url": server.url("/js")})),
        ("forms", json!({"url": server.url("/js")})),
    ] {
        let envelope = invoke(&mcp, tool, args).await;
        assert_degraded(&envelope, tool);
    }

    // 静态页 search 零命中：合法空结果，仍是 OK。
    let static_zero = invoke(
        &mcp,
        "search",
        json!({"url": server.url("/static"), "query": "no-such-needle"}),
    )
    .await;
    assert_eq!(static_zero["ok"], true, "{static_zero}");
    assert_eq!(code_of(&static_zero), "OK", "{static_zero}");
    assert!(
        static_zero["data"]["matches"]
            .as_array()
            .expect("matches")
            .is_empty(),
        "{static_zero}"
    );
}

/// #82：同一输入下 MCP 路径与库路径的 `code` 必须一致（不得各自为政）。
#[tokio::test]
async fn mcp_deg_code_matches_library_on_same_input() {
    let server = http_fixture::Server::start_with(vec![
        ("/js", http_fixture::Response::html(JS_SHELL)),
        ("/static", http_fixture::Response::html(STATIC_QUOTES)),
    ])
    .await;
    let mcp = WebctlMcp::new(in_process_context("deg-consistency"));
    let fetch_options = webctl_web::transport::FetchOptions::default();
    let extract_options = webctl_web::extract::ExtractOptions::default();
    let search_options = webctl_web::extract::SearchOptions::default();
    let selector = "div.quote";
    let query = "no-such-needle";

    for path in ["/js", "/static"] {
        let url = server.url(path);

        let mcp_fetch = invoke(&mcp, "fetch", json!({"url": url})).await;
        let library_fetch = webctl_web::transport::fetch(&url, &fetch_options).await;
        assert_eq!(
            mcp_fetch["code"],
            code_value(library_fetch.code),
            "fetch {path}: MCP 与库必须同码"
        );
        let text = library_fetch
            .data
            .expect("库取回 data")
            .content
            .text
            .expect("HTML 文本");

        let mcp_extract = invoke(&mcp, "extract", json!({"url": url, "selector": selector})).await;
        let library_extract =
            webctl_web::extract::select_matches(&text, selector, &extract_options);
        assert_eq!(
            mcp_extract["code"],
            code_value(library_extract.code),
            "extract {path}: MCP 与库必须同码"
        );

        let mcp_search = invoke(&mcp, "search", json!({"url": url, "query": query})).await;
        let library_search = webctl_web::extract::search(&text, query, &search_options);
        assert_eq!(
            mcp_search["code"],
            code_value(library_search.code),
            "search {path}: MCP 与库必须同码"
        );
    }
}

/// SEC-09：MCP 路径的页面内容标 untrusted，且信封不含凭据明文（接入 `redact_fetched`）。
#[tokio::test]
async fn sec_09_mcp_fetch_redacts_credentials() {
    const SECRET: &str = "SUPER_SECRET_TOKEN";
    let server = http_fixture::Server::start_with(vec![(
        "/login",
        http_fixture::Response::html("<html><body><main>login placeholder</main></body></html>")
            .header("set-cookie", &format!("sid={SECRET}; Path=/; HttpOnly"))
            .header("authorization", &format!("Bearer {SECRET}")),
    )])
    .await;
    let mcp = WebctlMcp::new(in_process_context("sec09-redact"));
    let url = format!("{}?access_token={SECRET}&user=bob", server.url("/login"));
    let envelope = invoke(&mcp, "fetch", json!({"url": url})).await;

    assert_eq!(envelope["meta"]["trust"], "untrusted_page_content");
    let serialized = serde_json::to_string(&envelope).expect("serialize");
    assert!(
        !serialized.contains(SECRET),
        "MCP 信封不得含凭据明文：{serialized}"
    );
    assert!(serialized.contains("<redacted>"), "{serialized}");
    assert!(
        serialized.contains("user=bob"),
        "非敏感 query 参数必须保留：{serialized}"
    );
}
