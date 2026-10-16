//! 安全与脱敏（PR 12 / 议题 #21；TESTING §2.14 SEC-*）。
//!
//! **默认严格**：本模块不提供任何“放宽”开关。所有对外可见的输出（结果信封、
//! 诊断日志、cookie 清单）都必须先经这里的原语处理；文件参数一律先做
//! canonicalize 与 containment 校验，非 http(s) 协议在 URL 层就被拒绝。
//!
//! 职责边界：
//! - `redact`：纯函数脱敏（cookie / Authorization / URL query token / 表单密码字段 / 日志行）。
//! - `paths`：文件参数 containment（复用 PR 05 的 canonicalize 原语）+ URL scheme 白名单。
//!
//! 本模块零子进程、无 unsafe、不联网。

pub mod paths;
pub mod redact;

pub use paths::{
    ArgumentError, SafeRoot, SchemeError, ensure_http_scheme, is_http_scheme, resolve_file_argument,
    scheme_of,
};
pub use redact::{
    REDACTED, is_password_field, is_sensitive_header, is_sensitive_param, redact_header_pairs,
    redact_header_value, redact_log_line, redact_query_string, redact_url, redact_urlencoded,
};
