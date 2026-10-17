//! MCP server 契约用例（MCP-01…MCP-14、ENV-06、STATE-06、SEC-09）。
//!
//! 两类驱动方式：
//! 1. 通过 CARGO_BIN_EXE_webctl-mcp 启动真实 stdio 进程，走 JSON-RPC（协议面用例）；
//! 2. 进程内直接调用 WebctlMcp::invoke / dispatch（零子进程与预算用例，MCP-09/MCP-14）。
//!
//! 硬约束：只访问本地文件与回环地址；运行期零子进程（仅测试夹具启动本仓二进制）。

#[path = "../../webctl-web/tests/support/http_fixture.rs"]
mod http_fixture;
#[path = "../../webctl-web/tests/support/resources.rs"]
mod resources;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use webctl_core::Config;
use webctl_mcp::{McpContext, TOOL_NAMES, WebctlMcp, qualified_name};

/// PR 09 的判据样本：JS 壳（内容只在内联脚本里）与静态对照页（正文充分）。
///
/// 直接复用 webctl-web 的 fixture，避免 MCP 侧出现第二套判定样本。
const JS_SHELL: &str =
    include_str!("../../webctl-web/tests/fixtures/degrade/js_data_script_shell.html");
const STATIC_QUOTES: &str =
    include_str!("../../webctl-web/tests/fixtures/degrade/static_quotes_10.html");

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("webctl-mcp-it-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn in_process_context(tag: &str) -> McpContext {
    let config = Config {
        state_dir: temp_dir(tag),
        ..Config::default()
    };
    McpContext::unbound(config)
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Server {
    fn start(state_dir: &Path, owner: Option<&str>) -> Self {
        let binary = env!("CARGO_BIN_EXE_webctl-mcp");
        let mut command = Command::new(binary);
        command
            .env("WEBCTL_STATE_DIR", state_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(owner) = owner {
            command.env("WEBCTL_MCP_SESSION", owner);
        }
        let _binary = env!("CARGO_BIN_EXE_webctl-mcp");
        let mut child = command.spawn().expect("spawn webctl-mcp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send(&mut self, value: Value) {
        writeln!(self.stdin, "{value}").expect("write request");
        self.stdin.flush().expect("flush request");
    }

    fn read_message(&mut self) -> Value {
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line).expect("read response");
        assert!(read > 0, "server closed stdout unexpectedly");
        serde_json::from_str(line.trim())
            .unwrap_or_else(|error| panic!("invalid JSON-RPC line {line:?}: {error}"))
    }

    fn read_until_id(&mut self, id: u64) -> Value {
        for _ in 0..64 {
            let value = self.read_message();
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return value;
            }
        }
        panic!("no response for id {id}");
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.read_until_id(id)
    }

    fn call(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        )
    }

    fn initialize(&mut self) -> Value {
        let response = self.request(
            1,
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "webctl-it", "version": "0.0.0"}
            }),
        );
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        response
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn envelope_of(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("tools/call 结果缺少文本内容：{response}"));
    serde_json::from_str(text).unwrap_or_else(|error| panic!("信封不是合法 JSON: {error}: {text}"))
}

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
