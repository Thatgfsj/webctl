//! webctl 的 MCP server 层（PLAN v0.4 §3.6 / §3.7）。
//!
//! 职责：
//! - `tools`：15 个工具的名字、说明与 schemars inputSchema（MCP-02…04）；
//! - `dispatch`：把工具调用接到 core / web 的既有能力，失败以信封返回（MCP-10）；
//! - `server`：rmcp ServerHandler + stdio 启动（MCP-01/05/06/09/11）；
//! - `ownership`：绑定 live Agent+Session，拒绝跨 Session（MCP-07）；
//! - `caps`：把实际注册情况翻译成 core 的能力报告（议题 #68）。
//!
//! 依赖方向：本 crate 依赖 core 与 web；core 不反向依赖本 crate。
//! 运行期零子进程：rmcp 只启用 server + stdio 传输，不引入任何子进程能力。
//!
//! # PR 12 → PR 11 交接点（SEC-09 脱敏）—— 仍未完成，后续 PR 补齐
//!
//! PR 12 提供 `webctl_core::security::redact` / `webctl_web::redact` 原语，并声明
//! SEC-09 的 MCP 侧接入（对工具结果调用 `webctl_web::redact::redact_fetched`，
//! 在序列化/内联/落盘前对 cookie 值、`Authorization`、URL query token、表单密码字段脱敏）
//! 归 PR 11。本 PR 的信封路径已对敏感响应头与 cookie 值做脱敏
//! （见 `dispatch::redact_fetch_headers`、`mask_state`、`write_cookie_export`），
//! 但**未**改接 PR 12 的 `redact_fetched`，也不宣称该交接点已完成。

pub mod caps;
pub mod context;
pub mod dispatch;
pub mod instructions;
pub mod ownership;
pub mod params;
pub mod server;
pub mod spill;
pub mod tools;

pub use context::McpContext;
pub use server::{DEFAULT_CALL_BUDGET_MS, WebctlMcp, run_stdio};
pub use tools::{TOOL_NAMES, TOOL_SPECS, ToolSpec, qualified_name, tool_definitions};

use webctl_core::{Config, ConfigLayer, resolve};

/// 环境变量：MCP server 绑定的会话名（由宿主/CLI 注入）。
pub const ENV_OWNER_SESSION: &str = "WEBCTL_MCP_SESSION";

/// 从环境变量读取绑定的会话名。
pub fn owner_from_env() -> Option<String> {
    std::env::var(ENV_OWNER_SESSION).ok()
}

/// 从环境变量层构造配置（配置文件层留空，独立二进制不读用户配置）。
///
/// 与 CLI 共享同一套 state dir 解析：`Config` 的 `state_dir` 来自 core 的
/// `default_state_dir()`（WEBCTL_STATE_DIR > HOME/.webctl），不自行拼路径。
pub fn config_from_environment() -> Config {
    let env_layer = ConfigLayer::from_env(|key| std::env::var(key).ok()).unwrap_or_default();
    resolve(&ConfigLayer::default(), &env_layer, &ConfigLayer::default())
}

/// crate 标识，供能力自检与诊断输出引用。
pub const CRATE: &str = "webctl-mcp";
