//! MCP-14 单独成二进制：本进程内只有这一个用例，子进程计数基线不受其他用例干扰。
//!
//! 断言方式与 PR 13 的观测工具一致：调用前后各采一次 child_process_count，增量必须为 0。
//! （进程内直接驱动 MCP 派发，而不是启动服务进程——启动服务进程本身会产生一个子进程。）

#[path = "../../webctl-web/tests/support/resources.rs"]
mod resources;

use serde_json::json;
use webctl_core::Config;
use webctl_mcp::{McpContext, TOOL_NAMES};

#[test]
fn mcp_14_in_process_dispatch_spawns_no_children() {
    let Some(before) = resources::child_process_count() else {
        let supported = resources::PLATFORM_SUPPORTED;
        assert!(!supported, "Linux 上必须给出子进程计数");
        return;
    };
    let config = Config {
        state_dir: std::env::temp_dir().join(format!("webctl-mcp14-{}", std::process::id())),
        ..Config::default()
    };
    let context = McpContext::unbound(config);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        for name in TOOL_NAMES {
            let _ = webctl_mcp::dispatch::dispatch(&context, name, json!({})).await;
        }
    });
    let after = resources::child_process_count().expect("第二次采样");
    assert_eq!(after, before, "MCP 派发不得创建子进程");
}
