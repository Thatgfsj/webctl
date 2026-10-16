//! 字符编码探测与解码（PLAN v0.4 §2「取回」域）。
//!
//! 优先级（确定性、可测）：
//! 1. BOM（UTF-8 / UTF-16LE / UTF-16BE）——最高优先；
//! 2. `Content-Type` 的 `charset` 参数；
//! 3. HTML `<meta charset>` / `<meta http-equiv=... content="...charset=...">`（前 4KB）；
//! 4. 无声明时：正文是合法 UTF-8 → UTF-8；否则 → windows-1252（WHATWG 传统回退）。
//!
//! 第 4 步是**有意的确定性回退**：不做语言猜测，不静默改用 GBK。
//! 调用方可以从 `DecodedText::source` 看到实际走了哪一步。

use encoding_rs::{Encoding, UTF_8, WINDOWS_1252};
use serde::{Deserialize, Serialize};

/// 实际采用的编码判定来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CharsetSource {
    /// 正文 BOM。
    Bom,
    /// `Content-Type: ...; charset=...`。
    ContentType,
    /// HTML `<meta charset>`。
    HtmlMeta,
    /// 无声明，且正文是合法 UTF-8。
    Utf8Valid,
    /// 无声明且非合法 UTF-8：确定性回退到 windows-1252。
    Fallback,
}

/// 解码结果与判定元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedText {
    /// 解码后的文本（替换非法序列，不报错）。
    pub text: String,
    /// 实际使用的编码名（如 `UTF-8` / `GBK` / `Shift_JIS`）。
    pub encoding: String,
    /// 判定来源。
    pub source: CharsetSource,
    /// 解码过程中是否出现过非法序列（被替换为 U+FFFD）。
    pub had_errors: bool,
}

/// 按固定优先级解码响应正文。
pub fn decode_body(body: &[u8], content_type: Option<&str>) -> DecodedText {
    if let Some((encoding, bom_len)) = Encoding::for_bom(body) {
        return decode_with(encoding, &body[bom_len..], CharsetSource::Bom);
    }
    if let Some(label) = content_type.and_then(charset_from_content_type)
        && let Some(encoding) = Encoding::for_label(label.as_bytes())
    {
        return decode_with(encoding, body, CharsetSource::ContentType);
    }
    if let Some(encoding) = sniff_meta_charset(body) {
        return decode_with(encoding, body, CharsetSource::HtmlMeta);
    }
    if std::str::from_utf8(body).is_ok() {
        return decode_with(UTF_8, body, CharsetSource::Utf8Valid);
    }
    decode_with(WINDOWS_1252, body, CharsetSource::Fallback)
}

/// 从 `Content-Type` 参数里取 `charset` 标签（原样，大小写不敏感匹配键）。
pub fn charset_from_content_type(content_type: &str) -> Option<String> {
    for param in content_type.split(';').skip(1) {
        let Some((key, value)) = param.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("charset") {
            continue;
        }
        let label = value.trim().trim_matches(|c| c == '"' || c == '\'');
        return if label.is_empty() {
            None
        } else {
            Some(label.to_owned())
        };
    }
    None
}

/// 在正文前 4KB 里找 HTML `<meta>` 声明的 charset。
pub fn sniff_meta_charset(body: &[u8]) -> Option<&'static Encoding> {
    let window = &body[..body.len().min(4096)];
    let text = String::from_utf8_lossy(window).to_ascii_lowercase();
    let mut rest = text.as_str();
    while let Some(position) = rest.find("<meta") {
        let tagged = &rest[position..];
        let end = tagged.find('>').unwrap_or(tagged.len());
        if let Some(label) = charset_in_tag(&tagged[..end])
            && let Some(encoding) = Encoding::for_label(label.as_bytes())
        {
            return Some(encoding);
        }
        rest = &rest[position + "<meta".len()..];
    }
    None
}

fn charset_in_tag(tag: &str) -> Option<String> {
    let after = &tag[tag.find("charset")? + "charset".len()..];
    let after_eq = &after[after.find('=')? + 1..];
    let value = after_eq.trim_start().trim_start_matches(['"', '\'']);
    let end = value
        .find(|c: char| c == '"' || c == '\'' || c == ';' || c == '>' || c.is_whitespace())
        .unwrap_or(value.len());
    let label = &value[..end];
    if label.is_empty() {
        None
    } else {
        Some(label.to_owned())
    }
}

fn decode_with(encoding: &'static Encoding, bytes: &[u8], source: CharsetSource) -> DecodedText {
    let (text, used, had_errors) = encoding.decode(bytes);
    DecodedText {
        text: text.into_owned(),
        encoding: used.name().to_string(),
        source,
        had_errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GBK_BYTES: &[u8] = &[
        0xd6, 0xd0, 0xce, 0xc4, 0xb1, 0xe0, 0xc2, 0xeb, 0xb2, 0xe2, 0xca, 0xd4, 0xa3, 0xba, 0xc4,
        0xe3, 0xba, 0xc3, 0xa3, 0xac, 0xca, 0xc0, 0xbd, 0xe7,
    ];
    const SJIS_BYTES: &[u8] = &[
        0x93, 0xfa, 0x96, 0x7b, 0x8c, 0xea, 0x82, 0xcc, 0x83, 0x65, 0x83, 0x58, 0x83, 0x67, 0x81,
        0x46, 0x82, 0xb1, 0x82, 0xf1, 0x82, 0xc9, 0x82, 0xbf, 0x82, 0xcd,
    ];

    /// HTTP-02：GBK。
    #[test]
    fn gbk_decodes() {
        let decoded = decode_body(GBK_BYTES, Some("text/html; charset=gbk"));
        assert_eq!(decoded.text, "中文编码测试：你好，世界");
        assert_eq!(decoded.encoding, "GBK");
        assert_eq!(decoded.source, CharsetSource::ContentType);
    }

    /// HTTP-03：Shift_JIS。
    #[test]
    fn shift_jis_decodes() {
        let decoded = decode_body(SJIS_BYTES, Some("text/html; charset=shift_jis"));
        assert_eq!(decoded.text, "日本語のテスト：こんにちは");
        assert_eq!(decoded.encoding, "Shift_JIS");
        assert_eq!(decoded.source, CharsetSource::ContentType);
    }

    /// HTTP-04：无声明时确定性回退。
    #[test]
    fn undeclared_body_falls_back_deterministically() {
        let utf8 = decode_body("你好".as_bytes(), Some("text/html"));
        assert_eq!(utf8.text, "你好");
        assert_eq!(utf8.source, CharsetSource::Utf8Valid);

        let undecodable = decode_body(GBK_BYTES, None);
        assert_eq!(undecodable.source, CharsetSource::Fallback);
        assert_eq!(undecodable.encoding, "windows-1252");
    }

    #[test]
    fn bom_wins_over_content_type() {
        let mut body = vec![0xef, 0xbb, 0xbf];
        body.extend_from_slice("hi".as_bytes());
        let decoded = decode_body(&body, Some("text/plain; charset=gbk"));
        assert_eq!(decoded.text, "hi");
        assert_eq!(decoded.source, CharsetSource::Bom);
    }

    #[test]
    fn meta_charset_is_used_when_header_has_none() {
        let mut body = b"<html><head><meta charset=\"gbk\"></head><body>".to_vec();
        body.extend_from_slice(GBK_BYTES);
        body.extend_from_slice(b"</body></html>");
        let decoded = decode_body(&body, Some("text/html"));
        assert_eq!(decoded.source, CharsetSource::HtmlMeta);
        assert_eq!(decoded.encoding, "GBK");
        assert!(decoded.text.contains("中文编码测试：你好，世界"));
    }

    #[test]
    fn charset_parser_handles_quotes_and_spacing() {
        assert_eq!(
            charset_from_content_type("text/html; charset=\"gbk\"").as_deref(),
            Some("gbk")
        );
        assert_eq!(
            charset_from_content_type("text/html;charset=utf-8").as_deref(),
            Some("utf-8")
        );
        assert_eq!(
            charset_from_content_type("text/html; boundary=x").as_deref(),
            None
        );
        assert_eq!(charset_from_content_type("text/html").as_deref(), None);
    }
}
