//! CSRF token 提取与回填（PLAN v0.4 §2「会话」域，PR 04）。
//!
//! 提取来源：form 内的 hidden/text 字段、页面 meta 标签。
//! 回填策略：提交前用**当前页面**里的 token 覆盖填充值，从而在 token 轮换后
//! 仍使用最新值（SESS-04 / SESS-05）；只覆盖该表单真实存在的字段，避免引入
//! 未知字段（FORM-07 要求未知字段必须报错而不是静默忽略）。

use serde::{Deserialize, Serialize};

use super::form::{Form, FormFill};
use super::html::{self, Node};

/// token 的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CsrfSource {
    /// 表单里的字段。
    FormField,
    /// 页面 meta 标签。
    MetaTag,
}

/// 一个提取到的 token。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CsrfToken {
    /// 字段名（或 meta 的 name）。
    pub name: String,
    /// token 值。
    pub value: String,
    /// 来源。
    pub source: CsrfSource,
}

/// 名字是否像 CSRF/防伪 token。
pub fn is_csrf_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }
    lower.contains("csrf")
        || lower.contains("xsrf")
        || lower == "_token"
        || lower == "token"
        || lower.ends_with("_token")
        || lower.ends_with("token")
        || lower.contains("authenticity_token")
        || lower.contains("requestverificationtoken")
        || lower.contains("antiforgery")
        || lower == "nonce"
}

/// 从一个表单里提取 token 字段（含隐藏与可见字段）。
pub fn extract_form_tokens(form: &Form) -> Vec<CsrfToken> {
    form.fields
        .iter()
        .filter(|field| is_csrf_name(&field.name) && !field.value.is_empty())
        .map(|field| CsrfToken {
            name: field.name.clone(),
            value: field.value.clone(),
            source: CsrfSource::FormField,
        })
        .collect()
}

/// 从页面里提取 token：先看各表单字段，再看 meta 标签。
pub fn extract_tokens(html: &str, forms: &[Form]) -> Vec<CsrfToken> {
    let mut tokens: Vec<CsrfToken> = Vec::new();
    for form in forms {
        for token in extract_form_tokens(form) {
            if !tokens.iter().any(|seen| seen.name == token.name) {
                tokens.push(token);
            }
        }
    }
    for token in extract_meta_tokens(html) {
        if !tokens.iter().any(|seen| seen.name == token.name) {
            tokens.push(token);
        }
    }
    tokens
}

/// 从 meta 标签提取 token（name/id 像 token 名，且带 content）。
pub fn extract_meta_tokens(html: &str) -> Vec<CsrfToken> {
    let nodes = html::tokenize(html);
    let mut tokens = Vec::new();
    for node in &nodes {
        let Node::Tag(tag) = node else { continue };
        if tag.closing || tag.name != "meta" {
            continue;
        }
        let name = tag.attr("name").or_else(|| tag.attr("id")).unwrap_or("");
        if !is_csrf_name(name) {
            continue;
        }
        let Some(content) = tag.attr("content").filter(|value| !value.is_empty()) else {
            continue;
        };
        tokens.push(CsrfToken {
            name: name.to_owned(),
            value: content.to_owned(),
            source: CsrfSource::MetaTag,
        });
    }
    tokens
}

/// 用页面里最新的 token 覆盖填充值（只覆盖表单真实存在的字段）。
///
/// 返回被回填的 token 数。
pub fn refill_from_html(fill: &mut FormFill, form: &Form, html: &str) -> usize {
    // 必须重新解析当前 HTML：form 对象里保存的是上一版字段值，直接用它会拿到旧 token。
    let fresh_forms = crate::session::form::parse_forms(html);
    let tokens = extract_tokens(html, &fresh_forms);
    let mut applied = 0usize;
    for token in tokens {
        if form.has_field(&token.name) {
            fill.set(token.name.clone(), token.value.clone());
            applied += 1;
        }
    }
    applied
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::form::parse_forms;

    #[test]
    fn extracts_hidden_and_meta_tokens() {
        let html = r#"<html><head><meta name="csrf-token" content="meta-1"></head>
            <body><form action="/x"><input type="hidden" name="csrf_token" value="field-1">
            <input name="user" value="u"></form></body></html>"#;
        let forms = parse_forms(html);
        let tokens = extract_tokens(html, &forms);
        assert!(tokens.iter().any(|token| token.name == "csrf_token"
            && token.value == "field-1"
            && token.source == CsrfSource::FormField));
        assert!(tokens.iter().any(|token| token.name == "csrf-token"
            && token.value == "meta-1"
            && token.source == CsrfSource::MetaTag));
    }

    #[test]
    fn refill_uses_latest_token_and_ignores_meta_only() {
        let html = r#"<html><head><meta name="csrf-token" content="meta-only"></head>
            <body><form action="/x"><input type="hidden" name="_token" value="old"></form></body></html>"#;
        let forms = parse_forms(html);
        let form = &forms[0];
        let mut fill = form.default_fill();
        assert_eq!(fill.get("_token"), Some("old"));
        let rotated = html.replace("value=\"old\"", "value=\"new\"");
        let applied = refill_from_html(&mut fill, form, &rotated);
        assert_eq!(applied, 1, "meta-only token 不应被塞进表单");
        assert_eq!(fill.get("_token"), Some("new"));
        assert!(!fill.contains("csrf-token"));
    }
}
