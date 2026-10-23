//! 浏览器上下文注入（0.2 M0 / ADR-0006）。
//!
//! 背景：webctl **不读取、不驱动**浏览器；只接受由宿主/LLM 从设备浏览器读取后传入的
//! `User-Agent` / `Accept*` / `Sec-CH-UA` / `Referer` / `Cookie`，**按单次请求生效**。
//!
//! 硬约束（必须由测试守住）：
//! - **Cookie 值绝不落盘、绝不落日志、绝不进 envelope**：`Debug` 实现只输出**条数**与域名，
//!   不输出任何 cookie 值；
//! - 诚实边界：注入只能借用 **HTTP 头与 Cookie**；**TLS 指纹仍是 wreq 的**、JS/WebGL/字体指纹
//!   **不存在** ✗ —— 这不是"伪装成浏览器"，只是"携带用户会话"；
//! - **刻意不实现 `Serialize`**：#161 复审实测，一旦可序列化，`serde_json::to_string` /
//!   `to_string_pretty` / `to_value` 就会把 cookie 值**明文**写进日志或 envelope ✗；
//!   因此只保留 `Deserialize` 供宿主传入，任何输出路径都必须走脱敏 `Debug`。
//! - 不保证绕过任何反爬；被拦即 `BLOCKED`（由上层判定）。

use std::fmt;

use serde::Deserialize;

/// 一条 cookie（仅在该域名下随请求发送）。
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct BrowserCookie {
    /// 域名（调用方保证只用于匹配的域）。
    pub domain: String,
    /// cookie 名。
    pub name: String,
    /// cookie 值——**敏感**，任何输出路径都必须脱敏。
    pub value: String,
}

/// 宿主提供的浏览器上下文（可选；缺省即不注入任何头）。
#[derive(Clone, Default, PartialEq, Eq, Deserialize)]
pub struct BrowserContext {
    /// `User-Agent`。
    pub user_agent: Option<String>,
    /// `Accept`。
    pub accept: Option<String>,
    /// `Accept-Language`。
    pub accept_language: Option<String>,
    /// `Sec-CH-UA`（Chromium Client Hints）。
    pub sec_ch_ua: Option<String>,
    /// `Referer`。
    pub referer: Option<String>,
    /// cookies（按域名发送）。
    #[serde(default)]
    pub cookies: Vec<BrowserCookie>,
}

impl BrowserContext {
    /// 是否为空（空则完全不改动请求）。
    pub fn is_empty(&self) -> bool {
        self.user_agent.is_none()
            && self.accept.is_none()
            && self.accept_language.is_none()
            && self.sec_ch_ua.is_none()
            && self.referer.is_none()
            && self.cookies.is_empty()
    }

    /// 需要注入的头部（**保序**；不含 `Host`/`Content-Length` 等由传输层决定的头）。
    ///
    /// Cookie 头只拼**域名匹配**的条目：`name=value; name2=value2`。
    pub fn header_pairs(&self, url_host: &str) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if let Some(v) = &self.user_agent {
            out.push(("user-agent", v.clone()));
        }
        if let Some(v) = &self.accept {
            out.push(("accept", v.clone()));
        }
        if let Some(v) = &self.accept_language {
            out.push(("accept-language", v.clone()));
        }
        if let Some(v) = &self.sec_ch_ua {
            out.push(("sec-ch-ua", v.clone()));
        }
        if let Some(v) = &self.referer {
            out.push(("referer", v.clone()));
        }
        let cookies: Vec<String> = self
            .cookies
            .iter()
            .filter(|c| domain_matches(&c.domain, url_host))
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();
        if !cookies.is_empty() {
            out.push(("cookie", cookies.join("; ")));
        }
        out
    }
}

/// 计算**实际要注入**的头部（纯函数，便于单测；不含任何 I/O）。
///
/// 规则（与 SEC-06「跨源重定向不得携带原始凭据」一致）：
/// - `user-agent` / `accept` / `accept-language` / `sec-ch-ua`：**总是**可带（非凭据）；
/// - `cookie` / `referer`：**只在同源时**携带 —— 跨源重定向一律丢弃 ✗；
/// - 显式 `user_agent` 参数**优先**于上下文（调用方已指定就不覆盖）；
/// - 上下文为空时返回空 vec（不产生任何变化）。
pub fn inject_headers(
    context: &BrowserContext,
    host: &str,
    same_origin: bool,
    explicit_user_agent: Option<&str>,
) -> Vec<(String, String)> {
    context
        .header_pairs(host)
        .into_iter()
        .filter(|(name, _)| {
            if *name == "cookie" || *name == "referer" {
                // 凭据类头：跨源不携带。
                same_origin
            } else {
                !(*name == "user-agent" && explicit_user_agent.is_some())
            }
        })
        .map(|(name, value)| (name.to_owned(), value))
        .collect()
}

/// cookie 域名是否匹配请求主机（`.example.com` 与 `example.com` 都匹配其子域与自身）。
fn domain_matches(cookie_domain: &str, host: &str) -> bool {
    let bare = cookie_domain.trim_start_matches('.').to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    if bare.is_empty() {
        return false;
    }
    host == bare || host.ends_with(&format!(".{bare}"))
}

/// **脱敏** `Debug`：绝不输出 cookie 值。
impl fmt::Debug for BrowserContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrowserContext")
            .field("user_agent", &self.user_agent.as_deref().map(|_| "<set>"))
            .field("accept", &self.accept.as_deref().map(|_| "<set>"))
            .field(
                "accept_language",
                &self.accept_language.as_deref().map(|_| "<set>"),
            )
            .field("sec_ch_ua", &self.sec_ch_ua.as_deref().map(|_| "<set>"))
            .field("referer", &self.referer.as_deref().map(|_| "<set>"))
            .field(
                "cookies",
                &format_args!("{} 条（已脱敏）", self.cookies.len()),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> BrowserContext {
        BrowserContext {
            user_agent: Some("Mozilla/5.0 (X11) Chrome/130".into()),
            accept: Some("text/html".into()),
            accept_language: Some("zh-CN".into()),
            sec_ch_ua: Some("\"Chromium\";v=\"130\"".into()),
            referer: Some("https://example.test/".into()),
            cookies: vec![
                BrowserCookie {
                    domain: ".example.test".into(),
                    name: "sid".into(),
                    value: "SECRET-1".into(),
                },
                BrowserCookie {
                    domain: "other.test".into(),
                    name: "x".into(),
                    value: "SECRET-2".into(),
                },
            ],
        }
    }

    #[test]
    fn header_pairs_map_to_expected_names() {
        let pairs = sample().header_pairs("www.example.test");
        let names: Vec<&str> = pairs.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            vec![
                "user-agent",
                "accept",
                "accept-language",
                "sec-ch-ua",
                "referer",
                "cookie"
            ]
        );
    }

    #[test]
    fn cookie_header_only_sends_matching_domains() {
        let pairs = sample().header_pairs("www.example.test");
        let cookie = pairs
            .iter()
            .find(|(n, _)| *n == "cookie")
            .expect("cookie header");
        assert!(
            cookie.1.contains("sid=SECRET-1"),
            "匹配域应发送：{}",
            cookie.1
        );
        assert!(
            !cookie.1.contains("SECRET-2"),
            "不匹配域不得发送：{}",
            cookie.1
        );
    }

    #[test]
    fn cookie_values_never_appear_in_debug() {
        let rendered = format!("{:?}", sample());
        assert!(
            !rendered.contains("SECRET-1"),
            "Debug 泄露 cookie 值：{rendered}"
        );
        assert!(
            !rendered.contains("SECRET-2"),
            "Debug 泄露 cookie 值：{rendered}"
        );
        assert!(
            !rendered.contains("sid="),
            "Debug 泄露 cookie 名值：{rendered}"
        );
        assert!(
            rendered.contains("2 条（已脱敏）"),
            "应显示条数：{rendered}"
        );
    }

    #[test]
    fn inject_headers_drops_credentials_on_cross_origin() {
        let pairs = inject_headers(&sample(), "example.test", false, None);
        let names: Vec<&str> = pairs.iter().map(|(n, _)| n.as_str()).collect();
        assert!(!names.contains(&"cookie"), "跨源不得携带 cookie：{names:?}");
        assert!(
            !names.contains(&"referer"),
            "跨源不得携带 referer：{names:?}"
        );
        assert!(names.contains(&"user-agent"), "非凭据头可跨源：{names:?}");
    }

    #[test]
    fn inject_headers_keeps_credentials_on_same_origin() {
        let pairs = inject_headers(&sample(), "www.example.test", true, None);
        let names: Vec<&str> = pairs.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"cookie"));
        assert!(names.contains(&"referer"));
    }

    #[test]
    fn explicit_user_agent_wins_over_context() {
        let pairs = inject_headers(&sample(), "example.test", true, Some("explicit/1.0"));
        assert!(
            pairs.iter().all(|(n, _)| n != "user-agent"),
            "显式 UA 优先，不应再注入上下文 UA"
        );
    }

    #[test]
    fn empty_context_produces_no_headers() {
        assert!(inject_headers(&BrowserContext::default(), "example.test", true, None).is_empty());
    }

    #[test]
    fn empty_context_injects_nothing() {
        let empty = BrowserContext::default();
        assert!(empty.is_empty());
        assert!(
            empty.header_pairs("example.test").is_empty(),
            "空上下文不得改动请求"
        );
    }

    #[test]
    fn domain_matching_handles_dot_and_subdomains() {
        assert!(domain_matches(".example.test", "a.b.example.test"));
        assert!(domain_matches("example.test", "example.test"));
        assert!(!domain_matches("example.test", "not-example.test"));
        assert!(!domain_matches("", "example.test"));
    }
}
