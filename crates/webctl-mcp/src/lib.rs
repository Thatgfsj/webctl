//! webctl 的 MCP server 层（PLAN v0.4 §3.6 / §3.7）。
//!
//! PR 01 只建立 crate 骨架；rmcp 依赖与 15 个工具在 PR 11 落地。

/// crate 标识，供能力自检与诊断输出引用。
pub const CRATE: &str = "webctl-mcp";
