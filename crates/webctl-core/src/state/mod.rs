//! 状态目录与状态文件基础设施（PLAN v0.4 §3.3，PR 05）。
//!
//! 这是 CLI 与 MCP 共享“同一份状态”的唯一入口：
//! - resolve_state_dir：state dir 解析（显式参数 > WEBCTL_STATE_DIR > HOME/.webctl > ./.webctl）；
//! - perms：目录 0700、文件 0600（非 Unix 显式降级为 None/no-op）；
//! - path：canonicalize 校验，拒绝 ../ 与越界 symlink；
//! - lock：flock 语义的 advisory 文件锁（带超时、无死锁）。
//!
//! 具体会话文件名与序列化在 webctl-web 的 session 模块；本模块只提供与业务
//! 无关的落盘原语，便于 PR 11/12 复用。
//!
//! 硬约束：零外部进程；本模块不含任何进程创建调用。

pub mod dir;
pub mod lock;
pub mod path;
pub mod perms;

#[cfg(test)]
pub(crate) mod testutil;

pub use dir::{STATE_DIR_ENV, STATE_DIR_NAME, resolve_state_dir, resolve_state_dir_from};
pub use lock::{DEFAULT_LOCK_POLL, DEFAULT_LOCK_WAIT, FileLock, LockError};
pub use path::{PathError, ensure_contained, is_within_lexically, safe_join, validate_segment};
pub use perms::{DIR_MODE, FILE_MODE, dir_mode, file_mode, mode_of, restrict_dir, restrict_file};
