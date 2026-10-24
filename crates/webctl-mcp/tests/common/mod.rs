//! MCP server 契约用例（MCP-01…MCP-14、ENV-06、STATE-06、SEC-09）。
//!
//! 两类驱动方式：
//! 1. 通过 CARGO_BIN_EXE_webctl-mcp 启动真实 stdio 进程，走 JSON-RPC（协议面用例）；
//! 2. 进程内直接调用 WebctlMcp::invoke / dispatch（零子进程与预算用例，MCP-09/MCP-14）。
//!
//! 硬约束：只访问本地文件与回环地址；运行期零子进程（仅测试夹具启动本仓二进制）。

#[path = "../../../webctl-web/tests/support/http_fixture.rs"]
pub mod http_fixture;
#[path = "../../../webctl-web/tests/support/resources.rs"]
pub mod resources;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use webctl_core::Config;
use webctl_mcp::McpContext;

/// PR 09 的判据样本：JS 壳（内容只在内联脚本里）与静态对照页（正文充分）。
///
/// 直接复用 webctl-web 的 fixture，避免 MCP 侧出现第二套判定样本。
pub const JS_SHELL: &str =
    include_str!("../../../webctl-web/tests/fixtures/degrade/js_data_script_shell.html");
pub const STATIC_QUOTES: &str =
    include_str!("../../../webctl-web/tests/fixtures/degrade/static_quotes_10.html");

pub fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("webctl-mcp-it-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

pub fn in_process_context(tag: &str) -> McpContext {
    let config = Config {
        state_dir: temp_dir(tag),
        ..Config::default()
    };
    McpContext::unbound(config)
}

pub struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Server {
    pub fn start(state_dir: &Path, owner: Option<&str>) -> Self {
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

    pub fn send(&mut self, value: Value) {
        writeln!(self.stdin, "{value}").expect("write request");
        self.stdin.flush().expect("flush request");
    }

    pub fn read_message(&mut self) -> Value {
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line).expect("read response");
        assert!(read > 0, "server closed stdout unexpectedly");
        serde_json::from_str(line.trim())
            .unwrap_or_else(|error| panic!("invalid JSON-RPC line {line:?}: {error}"))
    }

    pub fn read_until_id(&mut self, id: u64) -> Value {
        for _ in 0..64 {
            let value = self.read_message();
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return value;
            }
        }
        panic!("no response for id {id}");
    }

    pub fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.read_until_id(id)
    }

    pub fn call(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        )
    }

    pub fn initialize(&mut self) -> Value {
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

pub fn envelope_of(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("tools/call 结果缺少文本内容：{response}"));
    serde_json::from_str(text).unwrap_or_else(|error| panic!("信封不是合法 JSON: {error}: {text}"))
}
