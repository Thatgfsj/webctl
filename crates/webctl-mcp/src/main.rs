//! webctl-mcp：stdio MCP server 的独立入口。
//!
//! DSH 通常通过 `webctl mcp` 启动；本二进制用于独立挂载与 e2e 测试。
//! 硬约束：不启动任何子进程（PLAN v0.4 §3.1）。
use std::process::ExitCode;

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("webctl-mcp: 运行时时初始化失败：{error}");
            return ExitCode::FAILURE;
        }
    };
    let context = webctl_mcp::McpContext::new(
        webctl_mcp::config_from_environment(),
        webctl_mcp::owner_from_env(),
    );
    match runtime.block_on(webctl_mcp::run_stdio(context)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("webctl-mcp: {message}");
            ExitCode::FAILURE
        }
    }
}
