//! 运行时诊断日志（SEC-05 / SEC-07 / SEC-08）。
//!
//! **唯一出口**：任何写向 stderr 的诊断信息都必须经过这里，恒先调用
//! `webctl_core::security::redact::redact_log_line`。这样即使未来新增日志点，
//! 也不会绕过脱敏；`write_line` 暴露可注入的 sink 以便机械断言。
//!
//! 注意：结果信封的 JSON/文本输出不走这里（走 `cli::emit`），但同样脱敏。

use webctl_core::security::redact::redact_log_line;

/// 生成一行已脱敏的日志文本（不含换行）。
pub fn line(kind: &str, message: &str) -> String {
    format!("webctl: {kind}: {}", redact_log_line(message))
}

/// 写一行已脱敏日志到 stderr。
pub fn log(kind: &str, message: &str) {
    eprintln!("{}", line(kind, message));
}

/// 写到任意 sink（单测注入内存缓冲，验证脱敏确实生效）。
#[cfg(test)]
pub fn write_line<W: std::io::Write>(
    sink: &mut W,
    kind: &str,
    message: &str,
) -> std::io::Result<()> {
    writeln!(sink, "{}", line(kind, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(kind: &str, message: &str) -> String {
        let mut sink = Vec::new();
        write_line(&mut sink, kind, message).expect("write");
        String::from_utf8(sink).expect("utf8")
    }

    #[test]
    fn cookie_values_never_reach_the_log() {
        let logged = capture("headers", "Cookie: sid=SECRET; theme=light");
        assert!(!logged.contains("SECRET"), "{logged}");
        assert!(logged.contains("<redacted>"), "{logged}");
    }

    #[test]
    fn url_query_tokens_are_redacted_in_logs() {
        let logged = capture("fetch", "http://127.0.0.1:1/x?token=SECRET&user=bob");
        assert!(!logged.contains("SECRET"), "{logged}");
        assert!(logged.contains("token=<redacted>"), "{logged}");
        assert!(logged.contains("user=bob"), "{logged}");
    }

    #[test]
    fn password_form_fields_are_redacted_in_logs() {
        let logged = capture("form", "user=bob&password=SECRET&csrf=abc");
        assert!(!logged.contains("SECRET"), "{logged}");
        assert!(logged.contains("password=<redacted>"), "{logged}");
    }
}
