//! 最小 cookie jar（RFC 6265 存储与检索子集，PR 02）。
//!
//! **取舍**：不使用 wreq 的 `cookies` feature，也不直接依赖 `cookie` crate，
//! 而是手写最小实现。理由是：
//! 1. 需要有 serde 可往返的**中间结构**（session 持久化是 PR 04/05 的输入）；
//! 2. 约束（Domain / Path / Secure / 过期）需要逐条可测，不能藏在第三方默认值里；
//! 3. wreq 的 `Jar` 不暴露可序列化视图，而实现其 `CookieStore` trait 还要依赖其内部类型。
//!
//! 覆盖范围：`Domain` / `Path` / `Secure` / `HttpOnly` / `Max-Age`。
//! **有意不支持**：`Expires`（按会话 cookie 处理）、`SameSite`（无浏览器语义）、
//! 公有后缀表（不做 eTLD+1 特殊判断）、检索按插入序而非路径长度序。

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use url::Url;

/// 一条已存储的 cookie（可序列化的中间结构）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCookie {
    /// 名称。
    pub name: String,
    /// 值（未解码）。
    pub value: String,
    /// 小写、去前导点的域名。
    pub domain: String,
    /// 是否 host-only（`Set-Cookie` 未带 `Domain`）。
    pub host_only: bool,
    /// 路径。
    pub path: String,
    /// 仅 https 下发。
    pub secure: bool,
    /// 仅脚本不可见（本 jar 仍会下发，语义与浏览器一致）。
    pub http_only: bool,
    /// 过期时间（unix 秒）；`None` 为会话 cookie。
    pub expires_at: Option<u64>,
}

/// 最小的跨请求 cookie 存储。线程安全，可在多个请求间共享（`Arc<CookieJar>`）。
#[derive(Debug, Default)]
pub struct CookieJar {
    cookies: Mutex<Vec<StoredCookie>>,
}

impl CookieJar {
    /// 新建空 jar。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从序列化的中间结构恢复。
    pub fn from_snapshot(cookies: Vec<StoredCookie>) -> Self {
        Self {
            cookies: Mutex::new(cookies),
        }
    }

    /// 导出可序列化的中间结构（顺序与插入序一致）。
    pub fn snapshot(&self) -> Vec<StoredCookie> {
        self.lock().clone()
    }

    /// 当前存储条数（不含已过期的清理）。
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// 清空。
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// 批量吸收一组 `Set-Cookie` 头。
    pub fn store_set_cookies(&self, values: &[String], request_url: &Url) {
        for value in values {
            self.store(value, request_url);
        }
    }

    /// 吸收单条 `Set-Cookie`。Domain 不匹配或格式非法时静默忽略（HTTP 语义允许丢弃）。
    pub fn store(&self, raw: &str, request_url: &Url) {
        let Some(host) = request_url.host_str().map(str::to_ascii_lowercase) else {
            return;
        };
        let Some(parsed) = parse_set_cookie(raw) else {
            return;
        };
        let domain = match &parsed.domain {
            Some(domain) if domain_matches(&host, domain) => domain.clone(),
            // Domain 越界：拒绝，避免跨站注入。
            Some(_) => return,
            None => host,
        };
        let path = parsed
            .path
            .unwrap_or_else(|| default_path(request_url.path()));
        let now = now_secs();
        let expires_at = parsed.max_age.map(|seconds| {
            let absolute = i128::from(now) + i128::from(seconds);
            if absolute <= 0 {
                0
            } else {
                u64::try_from(absolute).unwrap_or(u64::MAX)
            }
        });

        let mut cookies = self.lock();
        if let Some(expires_at) = expires_at
            && expires_at <= now
        {
            cookies.retain(|cookie| {
                !(cookie.name == parsed.name && cookie.domain == domain && cookie.path == path)
            });
            return;
        }
        cookies.retain(|cookie| cookie.expires_at.is_none_or(|expires| expires > now));
        cookies.retain(|cookie| {
            !(cookie.name == parsed.name && cookie.domain == domain && cookie.path == path)
        });
        cookies.push(StoredCookie {
            name: parsed.name,
            value: parsed.value,
            domain,
            host_only: parsed.host_only,
            path,
            secure: parsed.secure,
            http_only: parsed.http_only,
            expires_at,
        });
    }

    /// 生成该 URL 的 `Cookie` 头；无可用 cookie 时返回 `None`。
    pub fn cookie_header(&self, url: &Url) -> Option<String> {
        let host = url.host_str()?.to_ascii_lowercase();
        let path = url.path();
        let secure = url.scheme() == "https";
        let now = now_secs();

        let mut cookies = self.lock();
        cookies.retain(|cookie| cookie.expires_at.is_none_or(|expires| expires > now));
        let mut header = String::new();
        for cookie in cookies
            .iter()
            .filter(|cookie| cookie_applies(cookie, &host, path, secure))
        {
            if !header.is_empty() {
                header.push_str("; ");
            }
            header.push_str(&cookie.name);
            header.push('=');
            header.push_str(&cookie.value);
        }
        if header.is_empty() {
            None
        } else {
            Some(header)
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<StoredCookie>> {
        // cookie jar 只做短临界区读写，中毒后继续用内部值而不是 panic。
        self.cookies
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug)]
struct ParsedSetCookie {
    name: String,
    value: String,
    domain: Option<String>,
    path: Option<String>,
    secure: bool,
    http_only: bool,
    host_only: bool,
    max_age: Option<i64>,
}

fn parse_set_cookie(raw: &str) -> Option<ParsedSetCookie> {
    let mut parts = raw.split(';');
    let pair = parts.next()?.trim();
    let (name, value) = pair.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let mut parsed = ParsedSetCookie {
        name: name.to_owned(),
        value: value.trim().to_owned(),
        domain: None,
        path: None,
        secure: false,
        http_only: false,
        host_only: true,
        max_age: None,
    };
    for attribute in parts {
        let attribute = attribute.trim();
        if attribute.eq_ignore_ascii_case("secure") {
            parsed.secure = true;
            continue;
        }
        if attribute.eq_ignore_ascii_case("httponly") {
            parsed.http_only = true;
            continue;
        }
        let Some((key, value)) = attribute.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.eq_ignore_ascii_case("domain") {
            let domain = value.trim_start_matches('.').to_ascii_lowercase();
            if !domain.is_empty() {
                parsed.domain = Some(domain);
                parsed.host_only = false;
            }
        } else if key.eq_ignore_ascii_case("path") {
            if value.starts_with('/') {
                parsed.path = Some(value.to_owned());
            }
        } else if key.eq_ignore_ascii_case("max-age")
            && let Ok(seconds) = value.parse::<i64>()
        {
            parsed.max_age = Some(seconds);
        }
        // Expires 有意忽略：按会话 cookie 处理（见模块文档）。
    }
    Some(parsed)
}

fn cookie_applies(cookie: &StoredCookie, host: &str, path: &str, secure: bool) -> bool {
    let domain_ok = if cookie.host_only {
        cookie.domain == host
    } else {
        domain_matches(host, &cookie.domain)
    };
    domain_ok && (!cookie.secure || secure) && path_matches(path, &cookie.path)
}

fn domain_matches(host: &str, domain: &str) -> bool {
    if host == domain {
        return true;
    }
    host.len() > domain.len()
        && host.ends_with(domain)
        && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
}

fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    cookie_path.ends_with('/') || request_path.as_bytes().get(cookie_path.len()) == Some(&b'/')
}

/// RFC 6265 §5.1.4 的默认 path。
fn default_path(request_path: &str) -> String {
    if !request_path.starts_with('/') {
        return "/".to_owned();
    }
    match request_path.rfind('/') {
        Some(index) if index > 0 => request_path[..index].to_owned(),
        _ => "/".to_owned(),
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(input: &str) -> Url {
        Url::parse(input).expect("test url")
    }

    /// HTTP-10：同宿主第二次请求带上第一次的 Set-Cookie。
    #[test]
    fn cookie_is_replayed_for_same_host() {
        let jar = CookieJar::new();
        jar.store("sid=abc123; Path=/", &url("http://example.test/login"));
        let header = jar
            .cookie_header(&url("http://example.test/next"))
            .expect("cookie");
        assert_eq!(header, "sid=abc123");
    }

    /// HTTP-11：Domain 越界不下发，Path 越界不下发，Secure 只在 https 下发。
    #[test]
    fn domain_path_secure_constraints_hold() {
        let jar = CookieJar::new();
        jar.store(
            "a=1; Domain=example.test; Path=/app",
            &url("http://example.test/app/x"),
        );
        jar.store("b=2; Path=/app", &url("http://example.test/app/x"));
        jar.store("s=3; Path=/; Secure", &url("https://example.test/"));

        // 越界域名：
        assert!(jar.cookie_header(&url("http://other.test/app/x")).is_none());
        // 越界路径：
        assert!(
            jar.cookie_header(&url("http://example.test/other"))
                .is_none()
        );
        // Secure 在 http 下不下发：
        let http_header = jar
            .cookie_header(&url("http://example.test/app/x"))
            .expect("http cookies");
        assert_eq!(http_header, "a=1; b=2");
        let https_header = jar
            .cookie_header(&url("https://example.test/app/x"))
            .expect("https cookies");
        assert_eq!(https_header, "a=1; b=2; s=3");
    }

    #[test]
    fn subdomain_domain_cookie_is_sent_and_rejected_cross_site() {
        let jar = CookieJar::new();
        // 父域声明的 Domain 对子域生效，但对相似后缀不生效。
        jar.store(
            "p=1; Domain=example.test; Path=/",
            &url("http://www.example.test/"),
        );
        assert_eq!(
            jar.cookie_header(&url("http://api.example.test/"))
                .as_deref(),
            Some("p=1")
        );
        assert!(jar.cookie_header(&url("http://notexample.test/")).is_none());
    }

    #[test]
    fn host_only_cookie_is_not_sent_to_subdomain() {
        let jar = CookieJar::new();
        jar.store("h=1; Path=/", &url("http://example.test/"));
        assert!(
            jar.cookie_header(&url("http://sub.example.test/"))
                .is_none()
        );
        assert!(jar.cookie_header(&url("http://example.test/")).is_some());
    }

    #[test]
    fn max_age_expiry_and_removal() {
        let jar = CookieJar::new();
        jar.store("gone=1; Path=/", &url("http://example.test/"));
        // Max-Age=0 表示删除。
        jar.store("gone=; Max-Age=0; Path=/", &url("http://example.test/"));
        assert!(jar.cookie_header(&url("http://example.test/")).is_none());
        assert!(jar.is_empty());
    }

    #[test]
    fn expired_cookie_is_pruned() {
        let jar = CookieJar::new();
        jar.store("t=1; Max-Age=1; Path=/", &url("http://example.test/"));
        {
            let mut cookies = jar.lock();
            cookies[0].expires_at = Some(1);
        }
        assert!(jar.cookie_header(&url("http://example.test/")).is_none());
        assert!(jar.is_empty());
    }

    #[test]
    fn httponly_and_secure_are_parsed() {
        let jar = CookieJar::new();
        jar.store(
            "x=1; Domain=.Example.test; Path=/a; Secure; HttpOnly",
            &url("https://example.test/a/b"),
        );
        let snapshot = jar.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert!(snapshot[0].http_only);
        assert!(snapshot[0].secure);
        assert!(!snapshot[0].host_only);
        assert_eq!(snapshot[0].domain, "example.test");
        assert_eq!(snapshot[0].path, "/a");
    }

    #[test]
    fn default_path_follows_rfc() {
        assert_eq!(default_path("/"), "/");
        assert_eq!(default_path("/app"), "/");
        assert_eq!(default_path("/app/page"), "/app");
        assert_eq!(default_path("/app/page/"), "/app/page");
        assert_eq!(default_path("relative"), "/");
    }

    #[test]
    fn jar_rejects_cookie_with_mismatched_domain() {
        let jar = CookieJar::new();
        jar.store(
            "evil=1; Domain=attacker.test; Path=/",
            &url("http://example.test/"),
        );
        assert!(jar.is_empty());
    }

    #[test]
    fn same_name_domain_path_is_overwritten() {
        let jar = CookieJar::new();
        jar.store("k=1; Path=/", &url("http://example.test/"));
        jar.store("k=2; Path=/", &url("http://example.test/"));
        assert_eq!(jar.len(), 1);
        assert_eq!(
            jar.cookie_header(&url("http://example.test/")).as_deref(),
            Some("k=2")
        );
    }
}
