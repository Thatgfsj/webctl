//! 凭据脱敏原语（SEC-04…SEC-08；TESTING §2.14）。
//!
//! 设计取舍：不引入正则依赖（core 保持零额外依赖），用可穷举的分词扫描实现；
//! 所有函数都是纯函数，输入输出可机械断言。默认严格：**未知字段宁可不放行**。
//!
//! 覆盖：
//! - `Cookie` / `Set-Cookie` 头：保留 cookie 名与属性，只脱敏值；
//! - `Authorization` / `Proxy-Authorization` / `X-Api-Key`：整体脱敏；
//! - URL query 与 `application/x-www-form-urlencoded` 表单体中的 token/secret/password 等；
//! - 日志行：先脱敏 URL、再脱敏头部、最后脱敏零散的 `key=value`。

/// 脱敏后的占位符（稳定字面量，测试与文档都引用它）。
pub const REDACTED: &str = "<redacted>";

/// 需要**整体**替换为 `<redacted>` 的凭据头（cookie 类单独处理以保留 cookie 名）。
const CREDENTIAL_HEADERS: [&str; 4] = [
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "x-auth-token",
];

/// 需要整体脱敏的 query/表单参数名（精确匹配，小写比较）。
const SENSITIVE_PARAM_EXACT: [&str; 14] = [
    "key", "pwd", "pw", "pass", "code", "sig", "jwt", "bearer", "csrf", "sid", "secret", "auth",
    "api-key", "access-key",
];

/// 需要脱敏的参数名片段（小写子串匹配）。
const SENSITIVE_PARAM_FRAGMENTS: [&str; 10] = [
    "token",
    "secret",
    "password",
    "passwd",
    "credential",
    "auth",
    "session",
    "jsession",
    "api_key",
    "apikey",
];

/// 该头部是否属于敏感凭据（大小写不敏感）。
pub fn is_sensitive_header(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    lower == "cookie" || lower == "set-cookie" || CREDENTIAL_HEADERS.contains(&lower.as_str())
}

/// 返回脱敏后的头部值；非敏感头部返回 `None`（调用方保持原值）。
pub fn redact_header_value(name: &str, value: &str) -> Option<String> {
    let lower = name.trim().to_ascii_lowercase();
    if lower == "cookie" {
        return Some(redact_cookie_pairs(value));
    }
    if lower == "set-cookie" {
        return Some(redact_set_cookie(value));
    }
    if CREDENTIAL_HEADERS.contains(&lower.as_str()) {
        return Some(REDACTED.to_owned());
    }
    None
}

/// 就地脱敏一组头部（重名、保序；非敏感头部保持不变）。
pub fn redact_header_pairs(headers: &mut [(String, String)]) {
    for (name, value) in headers.iter_mut() {
        if let Some(redacted) = redact_header_value(name, value) {
            *value = redacted;
        }
    }
}

/// `Cookie: a=1; b=2` → `a=<redacted>; b=<redacted>`。
pub fn redact_cookie_pairs(raw: &str) -> String {
    let mut parts = Vec::new();
    for part in raw.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('=') {
            Some((name, _)) if !name.trim().is_empty() => {
                parts.push(format!("{}={REDACTED}", name.trim()));
            }
            _ => parts.push(part.to_owned()),
        }
    }
    parts.join("; ")
}

/// `Set-Cookie: sid=abc; Path=/; HttpOnly` → `sid=<redacted>; Path=/; HttpOnly`。
pub fn redact_set_cookie(raw: &str) -> String {
    let mut parts = raw.split(';');
    let first = parts.next().unwrap_or("").trim();
    let head = match first.split_once('=') {
        Some((name, _)) if !name.trim().is_empty() => format!("{}={REDACTED}", name.trim()),
        _ if first.is_empty() => REDACTED.to_owned(),
        _ => REDACTED.to_owned(),
    };
    let attributes: Vec<&str> = parts.map(str::trim).filter(|part| !part.is_empty()).collect();
    if attributes.is_empty() {
        head
    } else {
        format!("{head}; {}", attributes.join("; "))
    }
}

/// 该参数名是否敏感（query / 表单通用，大小写不敏感）。
pub fn is_sensitive_param(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }
    if SENSITIVE_PARAM_EXACT.contains(&lower.as_str()) {
        return true;
    }
    SENSITIVE_PARAM_FRAGMENTS
        .iter()
        .any(|fragment| lower.contains(fragment))
}

/// 该表单字段是否密码类字段：type=password，或名字含 `passw`。
pub fn is_password_field(field_type: &str, name: &str) -> bool {
    field_type.trim().eq_ignore_ascii_case("password")
        || name.to_ascii_lowercase().contains("passw")
}

/// 脱敏 query 串（不含前导 `?`）：敏感参数的**值**替换为 `<redacted>`。
pub fn redact_query_string(query: &str) -> String {
    query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((name, _)) if is_sensitive_param(name) => format!("{}={REDACTED}", name),
            _ => pair.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// `application/x-www-form-urlencoded` 表单体与 query 同构，直接复用。
pub fn redact_urlencoded(body: &str) -> String {
    redact_query_string(body)
}

/// 脱敏 URL：先去掉 userinfo，再脱敏 query 中敏感参数的值；host/path 保留。
pub fn redact_url(raw: &str) -> String {
    let userinfo = crate::config::redact_url_userinfo(raw);
    let (head, fragment) = match userinfo.split_once('#') {
        Some((head, fragment)) => (head.to_owned(), Some(fragment.to_owned())),
        None => (userinfo, None),
    };
    let redacted = match head.split_once('?') {
        Some((base, query)) => format!("{base}?{}", redact_query_string(query)),
        None => head,
    };
    match fragment {
        Some(fragment) => format!("{redacted}#{fragment}"),
        None => redacted,
    }
}

/// 日志行统一入口：URL → 凭据头 → 零散 `key=value`。
pub fn redact_log_line(message: &str) -> String {
    let urls = redact_urls_in_text(message);
    let headers = redact_credential_headers_in_text(&urls);
    redact_pairs_in_text(&headers)
}

fn redact_urls_in_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while cursor < text.len() {
        let rest = &text[cursor..];
        let lower = rest.to_ascii_lowercase();
        let found = ["http://", "https://"]
            .iter()
            .filter_map(|scheme| lower.find(scheme))
            .min();
        let Some(offset) = found else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..offset]);
        let start = cursor + offset;
        let tail = &text[start..];
        let end = start + tail.find(is_url_delimiter).unwrap_or(tail.len());
        out.push_str(&redact_url(&text[start..end]));
        if end == cursor {
            // 理论不可达；防御性前进，避免死循环。
            out.push_str(&text[cursor..].chars().next().map_or("", |c| &text[cursor..cursor + c.len_utf8()]));
            cursor += 1;
        } else {
            cursor = end;
        }
    }
    out
}

fn is_url_delimiter(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>' | '`')
}

fn redact_credential_headers_in_text(text: &str) -> String {
    const NEEDLES: [&str; 6] = [
        "authorization:",
        "proxy-authorization:",
        "x-api-key:",
        "x-auth-token:",
        "set-cookie:",
        "cookie:",
    ];
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while cursor < text.len() {
        let rest = &text[cursor..];
        let lower = rest.to_ascii_lowercase();
        let found = NEEDLES
            .iter()
            .filter_map(|needle| lower.find(needle).map(|index| (index, needle.len())))
            .min_by_key(|(index, _)| *index);
        let Some((offset, length)) = found else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..offset]);
        let value_start = cursor + offset + length;
        out.push_str(&text[cursor + offset..value_start]);
        out.push_str(REDACTED);
        let value_end = text[value_start..]
            .find('\n')
            .map(|index| value_start + index)
            .unwrap_or(text.len());
        cursor = value_end;
        if value_end == value_start {
            cursor = value_start;
        }
    }
    out
}

fn redact_pairs_in_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(eq) = rest.find('=') {
        let key_start = rest[..eq]
            .rfind(|ch: char| !is_key_char(ch))
            .map(|index| index + 1)
            .unwrap_or(0);
        let key = &rest[key_start..eq];
        let value_end = rest[eq + 1..]
            .find(is_value_separator)
            .map(|index| eq + 1 + index)
            .unwrap_or(rest.len());
        if !key.is_empty() && (is_sensitive_param(key) || is_password_field("", key)) {
            out.push_str(&rest[..eq + 1]);
            out.push_str(REDACTED);
        } else {
            out.push_str(&rest[..value_end]);
        }
        rest = &rest[value_end..];
    }
    out.push_str(rest);
    out
}

fn is_key_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')
}

fn is_value_separator(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '&' | ';' | ',' | '"' | '\'' | '<' | '>' | ')' | ']')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_values_are_redacted_but_names_kept() {
        let header = redact_header_value("Cookie", "sid=abc123; theme=light").expect("sensitive");
        assert_eq!(header, "sid=<redacted>; theme=<redacted>");
        assert!(!header.contains("abc123"));

        let set_cookie =
            redact_header_value("Set-Cookie", "sid=abc123; Path=/; HttpOnly").expect("sensitive");
        assert!(set_cookie.starts_with("sid=<redacted>"));
        assert!(set_cookie.contains("HttpOnly"));
        assert!(!set_cookie.contains("abc123"));

        assert!(redact_header_value("Content-Type", "text/html").is_none());
    }

    #[test]
    fn authorization_is_fully_redacted() {
        assert_eq!(
            redact_header_value("Authorization", "Bearer SECRET").as_deref(),
            Some(REDACTED)
        );
        assert_eq!(
            redact_header_value("Proxy-Authorization", "Basic SECRET").as_deref(),
            Some(REDACTED)
        );
    }

    #[test]
    fn url_query_tokens_are_redacted() {
        let redacted = redact_url("https://example.test/login?user=bob&token=SECRET#frag");
        assert_eq!(
            redacted,
            "https://example.test/login?user=bob&token=<redacted>#frag"
        );
        assert!(is_sensitive_param("access_token"));
        assert!(is_sensitive_param("password"));
        assert!(!is_sensitive_param("user"));
    }

    #[test]
    fn log_line_redacts_urls_headers_and_form_fields() {
        let line = redact_log_line("GET https://example.test/x?token=SECRET&user=bob HTTP/1.1");
        assert!(!line.contains("SECRET"), "{line}");
        assert!(line.contains("token=<redacted>"), "{line}");

        let cookie = redact_log_line("request Cookie: sid=SECRET; theme=light");
        assert!(!cookie.contains("SECRET"), "{cookie}");

        let form = redact_log_line("form body: user=bob&password=SECRET&csrf=abc");
        assert!(!form.contains("SECRET"), "{form}");
    }
}
