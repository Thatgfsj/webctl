//! webctl 的 MCP server 层（PLAN v0.4 §3.6 / §3.7）。
//!
//! PR 01 只建立 crate 骨架；rmcp 依赖与 15 个工具在 PR 11 落地。
//!
//! # PR 12 → PR 11 的显式交接点（SEC-09）
//!
//! TESTING §2.14 的 SEC-09 归 PR 11：**MCP 路径返回的页面内容同样必须标
//! `Trust::UntrustedPageContent`**，且在序列化/内联/落盘前必须经过
//! `webctl_core::security::redact`（cookie 值、`Authorization`、URL query token、
//! 表单密码字段）脱敏。PR 12 只提供这些原语与信封/日志接入点，
//! **不在本 crate 内实现 MCP 工具序列化入口**，以免与并行开发的 MCP 分支冲突。
//! TODO(PR 11)：在工具结果组装处调用 `webctl_web::redact::redact_fetched` /
//! `redact_log_line`，并为 MCP 侧补 SEC-09 断言。

/// crate 标识，供能力自检与诊断输出引用。
pub const CRATE: &str = "webctl-mcp";
