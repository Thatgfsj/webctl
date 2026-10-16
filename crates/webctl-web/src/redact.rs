//! 信封 / 会话状态的脱敏接入点（SEC-04；TESTING §2.14）。
//!
//! 纯脱敏原语在 `webctl_core::security::redact`；本模块负责把它们接到
//! 取回结果与会话状态上。**默认严格**：transport 在 `finish` 里就调用
//! `redact_fetched`，因此任何上层（CLI / MCP / 未来接口）拿到的信封
//! 都不会含 cookie / Authorization 明文，而不是“谁输出谁记得脱敏”。
//!
//! 注意边界：cookie jar 的**存储**仍是明文（会话持久化需要），但任何
//! 对外序列化（`cookies` 清单、日志、信封）都必须经过本模块或核心原语。

pub use webctl_core::security::redact::*;

use crate::session::SessionState;
use crate::transport::{Fetched, StoredCookie};

/// 就地脱敏一次取回结果：敏感头、`final_url` 与重定向链中的 query token。
pub fn redact_fetched(fetched: &mut Fetched) {
    redact_header_pairs(&mut fetched.headers);
    fetched.final_url = redact_url(&fetched.final_url);
    for hop in &mut fetched.redirects {
        hop.url = redact_url(&hop.url);
        hop.location = redact_url(&hop.location);
    }
}

/// 就地脱敏 cookie 快照（值置为 `<redacted>`，保留名字与作用域）。
pub fn redact_stored_cookies(cookies: &mut [StoredCookie]) {
    for cookie in cookies {
        cookie.value = REDACTED.to_owned();
    }
}

/// 会话状态用于展示时的脱敏副本。
pub fn redact_session_state(state: &mut SessionState) {
    redact_stored_cookies(&mut state.cookies);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::RedirectHop;

    fn fetched() -> Fetched {
        Fetched {
            status: 200,
            version: "http/1.1".to_owned(),
            final_url: "https://example.test/z?token=SECRET&user=bob".to_owned(),
            headers: vec![
                ("set-cookie".to_owned(), "sid=SECRET; Path=/".to_owned()),
                ("content-type".to_owned(), "text/html".to_owned()),
                ("authorization".to_owned(), "Bearer SECRET".to_owned()),
            ],
            redirects: vec![RedirectHop {
                url: "https://example.test/a?access_token=SECRET".to_owned(),
                status: 302,
                location: "https://example.test/z?token=SECRET".to_owned(),
            }],
            content: crate::transport::Body {
                kind: crate::content_type::ContentKind::Text,
                content_type: Some("text/html".to_owned()),
                byte_len: 0,
                declared_len: Some(0),
                truncated: false,
                text: Some("ok".to_owned()),
                charset: Some("UTF-8".to_owned()),
                charset_source: None,
                decode_had_errors: false,
            },
            elapsed_ms: 1,
        }
    }

    #[test]
    fn envelope_redaction_removes_secrets_everywhere() {
        let mut data = fetched();
        redact_fetched(&mut data);
        let serialized = serde_json::to_string(&data).expect("serialize");
        assert!(!serialized.contains("SECRET"), "{serialized}");
        assert!(serialized.contains("token=<redacted>"), "{serialized}");
        assert!(serialized.contains("user=bob"), "{serialized}");
        assert!(
            data.headers
                .iter()
                .any(|(name, value)| name == "content-type" && value == "text/html"),
            "非敏感头必须保持原样"
        );
    }

    #[test]
    fn session_state_display_never_contains_cookie_values() {
        let mut state = SessionState::new("default", std::time::Duration::from_secs(60));
        state.cookies.push(StoredCookie {
            name: "sid".to_owned(),
            value: "SECRET".to_owned(),
            domain: "example.test".to_owned(),
            host_only: true,
            path: "/".to_owned(),
            secure: true,
            http_only: true,
            expires_at: None,
        });
        redact_session_state(&mut state);
        assert_eq!(state.cookies[0].value, REDACTED);
        assert_eq!(state.cookies[0].name, "sid");
    }
}
