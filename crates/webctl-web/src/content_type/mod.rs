//! 内容类型路由（PLAN v0.4 §2「取回」域）。
//!
//! 把取回的字节明确分成 html / json / xml / pdf / binary 五条分支。
//! 二进制（含 PDF）**永不**当作文本解析：只保留长度与类型元数据，
//! 正文提取留给 PR 03 / PR 07。

use serde::{Deserialize, Serialize};

/// 取回结果的内容分支。序列化为小写下划线字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    /// `text/html`、`application/xhtml+xml` 或嗅探为 HTML。
    Html,
    /// `application/json`、`*+json`。
    Json,
    /// `application/xml`、`text/xml`、`*+xml`。
    Xml,
    /// `application/pdf`。
    Pdf,
    /// 其余一切类型；按字节原样返回，不做文本解析。
    Binary,
}

impl ContentKind {
    /// 是否按文本解码（html/json/xml 为真，pdf/binary 为假）。
    pub fn is_text(self) -> bool {
        matches!(self, Self::Html | Self::Json | Self::Xml)
    }

    /// 稳定的线上字符串（与 serde 重命名一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Json => "json",
            Self::Xml => "xml",
            Self::Pdf => "pdf",
            Self::Binary => "binary",
        }
    }
}

/// 从 `Content-Type` 头判定分支；头缺失或为 `text/plain` 时按正文嗅探。
pub fn classify(content_type: Option<&str>, body: &[u8]) -> ContentKind {
    match content_type.and_then(mime_of) {
        Some(mime) if is_html(&mime) => ContentKind::Html,
        Some(mime) if is_json(&mime) => ContentKind::Json,
        Some(mime) if is_xml(&mime) => ContentKind::Xml,
        Some(mime) if mime == "application/pdf" => ContentKind::Pdf,
        // text/plain 与未知 MIME 一样，交给嗅探决定；嗅探不出即 binary。
        Some(mime) if mime == "text/plain" => sniff(body),
        Some(_) => ContentKind::Binary,
        None => sniff(body),
    }
}

/// 取 MIME 主类型：小写、去参数；空值返回 `None`。
pub fn mime_of(content_type: &str) -> Option<String> {
    let mime = content_type.split(';').next()?.trim().to_ascii_lowercase();
    if mime.is_empty() { None } else { Some(mime) }
}

fn is_html(mime: &str) -> bool {
    mime == "text/html" || mime == "application/xhtml+xml"
}

fn is_json(mime: &str) -> bool {
    mime == "application/json" || mime == "text/json" || mime.ends_with("+json")
}

fn is_xml(mime: &str) -> bool {
    mime == "application/xml" || mime == "text/xml" || mime.ends_with("+xml")
}

/// 无可用 `Content-Type` 时的确定性嗅探（只看前 1KB）。
fn sniff(body: &[u8]) -> ContentKind {
    let window = &body[..body.len().min(1024)];
    let text = String::from_utf8_lossy(window);
    let trimmed = text.trim_start();
    if trimmed.starts_with("%PDF-") {
        return ContentKind::Pdf;
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return ContentKind::Json;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("<?xml") {
        return ContentKind::Xml;
    }
    if lower.starts_with("<!doctype html") || lower.contains("<html") {
        return ContentKind::Html;
    }
    ContentKind::Binary
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HTTP-15：五个分支各自命中。
    #[test]
    fn five_branches_are_distinct() {
        assert_eq!(
            classify(Some("text/html; charset=utf-8"), b"<html></html>"),
            ContentKind::Html
        );
        assert_eq!(
            classify(Some("application/json"), b"{\"a\":1}"),
            ContentKind::Json
        );
        assert_eq!(classify(Some("application/xml"), b"<r/>"), ContentKind::Xml);
        assert_eq!(
            classify(Some("application/pdf"), b"%PDF-1.7"),
            ContentKind::Pdf
        );
        assert_eq!(
            classify(Some("image/png"), &[0x89, b'P', b'N', b'G']),
            ContentKind::Binary
        );
    }

    #[test]
    fn suffix_types_are_recognized() {
        assert_eq!(
            classify(Some("application/vnd.api+json"), b""),
            ContentKind::Json
        );
        assert_eq!(classify(Some("image/svg+xml"), b""), ContentKind::Xml);
        assert_eq!(
            classify(Some("application/xhtml+xml"), b""),
            ContentKind::Html
        );
    }

    /// 缺失或 text/plain 的 Content-Type 走嗅探；嗅探不出即 binary。
    #[test]
    fn missing_content_type_sniffs() {
        assert_eq!(
            classify(None, b"  <!DOCTYPE html><html></html>"),
            ContentKind::Html
        );
        assert_eq!(
            classify(None, b"  <?xml version=\"1.0\"?><a/>"),
            ContentKind::Xml
        );
        assert_eq!(classify(None, b"[1,2,3]"), ContentKind::Json);
        assert_eq!(classify(None, b"%PDF-1.4"), ContentKind::Pdf);
        assert_eq!(classify(None, &[0x00, 0x01, 0x02]), ContentKind::Binary);
        assert_eq!(
            classify(Some("text/plain"), b"<html><body>x</body></html>"),
            ContentKind::Html
        );
        assert_eq!(
            classify(Some("text/plain"), b"just words"),
            ContentKind::Binary
        );
    }

    /// 二进制绝不被当作文本。
    #[test]
    fn binary_is_not_text() {
        assert!(!ContentKind::Binary.is_text());
        assert!(!ContentKind::Pdf.is_text());
        assert!(
            ContentKind::Html.is_text()
                && ContentKind::Json.is_text()
                && ContentKind::Xml.is_text()
        );
    }

    #[test]
    fn kind_strings_are_stable() {
        let value = serde_json::to_value(ContentKind::Pdf).unwrap();
        assert_eq!(value, serde_json::json!("pdf"));
        assert_eq!(ContentKind::Html.as_str(), "html");
    }
}
