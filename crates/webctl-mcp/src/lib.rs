//! webctl 的 MCP server 层（PLAN v0.4 §3.6 / §3.7）。
//!
//! 职责：
//! - `tools`：17 个工具的名字、说明与 schemars inputSchema（MCP-02…04）；
//! - `dispatch`：把工具调用接到 core / web 的既有能力，失败以信封返回（MCP-10）；
//! - `server`：rmcp ServerHandler + stdio 启动（MCP-01/05/06/09/11）；
//! - `ownership`：绑定 live Agent+Session，拒绝跨 Session（MCP-07）；
//! - `caps`：把实际注册情况翻译成 core 的能力报告（议题 #68）；
//! - `site`：`resolve_site` 工具的实现（0.2 M1，ADR-0006；单独成文件以免 dispatch 破 1000 行）。
//!
//! 依赖方向：本 crate 依赖 core 与 web；core 不反向依赖本 crate。
//! 运行期零子进程：rmcp 只启用 server + stdio 传输，不引入任何子进程能力。
//!
//! # SEC-09（MCP 侧脱敏与信任标记）—— 已接入
//!
//! 页面/网络内容一律 `trust = untrusted_page_content`；只有 caps 这种本地自检
//! 数据用 `trusted_local`。取回结果在进入信封 / 序列化 / 落盘前经
//! `webctl_web::redact::redact_fetched` 脱敏（cookie 值、`Authorization`、
//! URL query token、重定向链）。取回层（`transport::finish`）与本层
//! （`dispatch::redact_fetched_envelope`）各显式接一次；会话状态与 cookie 导出
//! 另有 `dispatch::mask_state` / `write_cookie_export`。

pub mod caps;
pub mod context;
mod degradation;
pub mod dispatch;
mod embedded_tool;
pub mod instructions;
pub mod ownership;
pub mod params;
pub mod server;
mod site;
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
