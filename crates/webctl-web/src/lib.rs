//! webctl 内置层：取回 + 解析 + 会话 + 文档与数据（PLAN v0.4 §2）。
//!
//! PR 01 只建立 crate 骨架，本层从 PR 02 起逐步落地。
//! 硬约束：零外部进程、零浏览器（PLAN v0.4 §3.1）。

/// crate 标识，供能力自检与诊断输出引用。
pub const CRATE: &str = "webctl-web";
