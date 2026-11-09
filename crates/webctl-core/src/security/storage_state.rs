//! storage_state 导出的脱敏辅助（0.2.3 M6 / #226）。
//!
//! 本模块只放**纯函数**，不依赖会话、文件或网络；职责是把可能进入日志的
//! 凭据替换成固定占位符。设计取舍与 `security::redact` 一致：默认严格，
//! 不提供"保留原值"的开关。
//!
//! 重要边界：storage_state 是**导出给 Playwright 复用登录态**的凭据文件，
//! 其内容必须原样写入 0600 文件；本模块只服务于**日志/诊断**，绝不能被
//! 用来"脱敏"落盘内容，否则 Playwright 无法复用。

/// 进入日志的 cookie 值与 storage 值统一替换为该占位符。
pub const LOG_REDACTED: &str = "***";

/// 返回可安全写入日志的值：无论输入是什么，都只返回 `***`。
///
/// 保留"值"这一参数而不是让调用方直接写常量，是为了让调用点显式表达
/// "这里原本是一个凭据值"，避免未来有人误把原值拼进日志。
pub fn redact_for_log(_value: &str) -> String {
    LOG_REDACTED.to_owned()
}

/// `name=value` → `name=***`：保留 cookie/storage 名用于定位，值一律隐藏。
pub fn redact_cookie_pair_for_log(name: &str, _value: &str) -> String {
    format!("{name}={LOG_REDACTED}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_values_are_replaced_by_placeholder() {
        let rendered = redact_for_log("SESSION_SECRET_VALUE");
        assert_eq!(rendered, LOG_REDACTED);
        assert!(!rendered.contains("SESSION_SECRET_VALUE"));
        assert_eq!(redact_for_log(""), LOG_REDACTED);
    }

    #[test]
    fn cookie_pair_keeps_name_but_hides_value() {
        let rendered = redact_cookie_pair_for_log("sid", "SESSION_SECRET_VALUE");
        assert_eq!(rendered, "sid=***");
        assert!(!rendered.contains("SESSION_SECRET_VALUE"));
    }
}
