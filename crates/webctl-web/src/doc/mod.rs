//! 文档与数据内置处理（PLAN v0.4 §2 能力域「文档/数据」，PR 07）。
//!
//! 本模块把 v0.3 依赖外部 CLI 的能力改成进程内实现：
//!
//! - pdf::extract_pdf_text：PDF 文本层（替代 pdftotext）；
//! - json_query::json_query：JSON 查询子集（替代 jq）。
//!
//! # DOC-09（已达成）
//!
//! TESTING §2.9 的 DOC-09 要求「HTML→文本复用解析层，结果与 PARSE-01 一致」。
//! PR 03 解析层合并（`5afc240`）后，本模块的 Html/Xml 分支已改为直接调用解析层：
//! HTML 走 `parse::extract_article`（正文纯文本），XML 走 `parse::parse_xml` 后只收文本节点。
//! **本模块不重复实现标记语言解析**，也不再用 `UNSUPPORTED` 假装「能力不足」。
//! 交接期的降级标识（`parse-layer-pending-pr03`）已从代码与测试中彻底删除。
//!
//! # 硬约束
//!
//! - 零子进程：只用纯 Rust 解析库，代码里不出现 std::process / Command；
//! - 所有返回走 webctl_core::Envelope，页面内容固定 Trust::UntrustedPageContent；
//! - 禁止静默降级：UNSUPPORTED / LIMIT 一律带 meta.degraded_from；
//! - 四类失败可区分：空结果（OK 且 matched 0）、解析失败（PARSE）、
//!   能力不支持（UNSUPPORTED）、被拦截（BLOCKED，本模块不产生）。

pub mod json_query;
pub mod pdf;

pub use json_query::{DEFAULT_MAX_JSON_BYTES, JsonQueryOptions, JsonQueryOutcome, json_query};
pub use pdf::extract_pdf_text;

use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use webctl_core::{Envelope, ErrorCode, Trust};

/// 文档输入上限（8 MiB）：超限只返回 LIMIT，不进入任何解析。
pub const DEFAULT_MAX_DOC_BYTES: usize = 8 * 1024 * 1024;

/// doc_text 参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocOptions {
    /// 允许解析的最大输入字节数。
    pub max_bytes: usize,
}

impl Default for DocOptions {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_DOC_BYTES,
        }
    }
}

/// 文档格式判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocFormat {
    /// PDF（%PDF- 魔数）。
    Pdf,
    /// JSON（首个非空白字节是 { 或 [）。
    Json,
    /// XML（<?xml 或一般标签开头）。
    Xml,
    /// HTML（<!doctype html 或含 <html）。
    Html,
    /// 合法 UTF-8 且无二进制控制字符的纯文本。
    Text,
    /// 其余一切字节；doc_text 对它返回 UNSUPPORTED。
    Unknown,
}

impl DocFormat {
    /// 稳定的线上字符串（与 serde 重命名一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Json => "json",
            Self::Xml => "xml",
            Self::Html => "html",
            Self::Text => "text",
            Self::Unknown => "unknown",
        }
    }
}

/// PDF 子类型：让「成功 / 无文本层 / 加密 / 无页 / 损坏」互相区分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PdfKind {
    /// 成功提取到文本层。
    TextLayer,
    /// 有页面但没有可提取文本（扫描件/纯图片）。
    ScannedOrImageOnly,
    /// 带 /Encrypt 的加密 PDF。
    Encrypted,
    /// 没有页面对象。
    NoPages,
    /// 结构损坏；只会出现在失败信封的诊断里。
    Malformed,
}

impl PdfKind {
    /// 稳定的线上字符串（与 serde 重命名一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TextLayer => "text_layer",
            Self::ScannedOrImageOnly => "scanned_or_image_only",
            Self::Encrypted => "encrypted",
            Self::NoPages => "no_pages",
            Self::Malformed => "malformed",
        }
    }
}

/// 文档文本层结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocText {
    /// 判定出的格式。
    pub format: DocFormat,
    /// 提取出的文本。成功时非空；不支持时为 None（不返回空字符串假装成功）。
    pub text: Option<String>,
    /// PDF 页数（仅 PDF）。
    pub pages: Option<usize>,
    /// PDF 子类型（仅 PDF）。
    pub pdf_kind: Option<PdfKind>,
    /// 人类可读诊断，说明为何 UNSUPPORTED 或 LIMIT。
    pub diagnostics: Vec<String>,
}

impl DocText {
    /// 构造一个「不支持」结果：text 恒为 None，诊断必须非空。
    pub(crate) fn unsupported(
        format: DocFormat,
        pdf_kind: Option<PdfKind>,
        diagnostics: Vec<String>,
    ) -> Self {
        debug_assert!(!diagnostics.is_empty(), "UNSUPPORTED 必须有可读诊断");
        Self {
            format,
            text: None,
            pages: None,
            pdf_kind,
            diagnostics,
        }
    }
}

/// 按魔数/结构判定文档格式（只看前 1KB，确定性、不联网、不起子进程）。
pub fn detect_format(bytes: &[u8]) -> DocFormat {
    if bytes.is_empty() {
        return DocFormat::Unknown;
    }
    let head = &bytes[..bytes.len().min(1024)];
    if head.windows(5).any(|window| window == b"%PDF-") {
        return DocFormat::Pdf;
    }
    let text = String::from_utf8_lossy(head);
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return DocFormat::Json;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("<!doctype html") || lower.contains("<html") {
        return DocFormat::Html;
    }
    if lower.starts_with("<?xml") || trimmed.starts_with('<') {
        return DocFormat::Xml;
    }
    if std::str::from_utf8(bytes).is_ok()
        && !bytes
            .iter()
            .any(|byte| *byte < 0x20 && !matches!(*byte, b'\n' | b'\r' | b'\t'))
    {
        return DocFormat::Text;
    }
    DocFormat::Unknown
}

/// 文档文本层提取入口（PLAN §3.7 工具 12 doc_text 的内核）。
///
/// - 输入超限 -> LIMIT（degraded_from == "input-size-limit"），不解析、不 OOM；
/// - PDF -> pdf::extract_pdf_text；
/// - 纯文本 -> OK；
/// - HTML / XML -> **交给解析层**（`parse::extract_article` / `parse::parse_xml`）：失败透传其 code/error，降级保留其 degraded_from；
/// - JSON -> UNSUPPORTED（degraded_from == "use-json-query"），请改用 json_query，本层不重复解析；
/// - 未知 -> UNSUPPORTED，且带可读诊断与 degraded_from。
pub fn doc_text(bytes: &[u8], options: &DocOptions) -> Envelope<DocText> {
    let started = Instant::now();
    let elapsed = || started.elapsed().as_millis() as u64;

    if bytes.len() > options.max_bytes {
        return Envelope::degraded(
            DocText::unsupported(
                DocFormat::Unknown,
                None,
                vec![format!(
                    "输入 {} 字节，超过上限 {} 字节，未解析",
                    bytes.len(),
                    options.max_bytes
                )],
            ),
            "input-size-limit",
            ErrorCode::Limit,
            elapsed(),
        );
    }

    let format = detect_format(bytes);
    match format {
        DocFormat::Pdf => extract_pdf_text(bytes, options.max_bytes),
        DocFormat::Text => match std::str::from_utf8(bytes) {
            Ok(text) => Envelope::ok(
                DocText {
                    format: DocFormat::Text,
                    text: Some(text.to_string()),
                    pages: None,
                    pdf_kind: None,
                    diagnostics: Vec::new(),
                },
                Trust::UntrustedPageContent,
                elapsed(),
            ),
            Err(error) => Envelope::fail(
                ErrorCode::Parse,
                format!("文本解码失败：{error}"),
                elapsed(),
            ),
        },
        DocFormat::Json => Envelope::degraded(
            DocText::unsupported(
                DocFormat::Json,
                None,
                vec!["JSON 是数据不是文档文本；请使用 json_query 而不是 doc_text".to_string()],
            ),
            "use-json-query",
            ErrorCode::Unsupported,
            elapsed(),
        ),
        DocFormat::Html | DocFormat::Xml => markup_text(bytes, format, elapsed()),
        DocFormat::Unknown => Envelope::degraded(
            DocText::unsupported(
                DocFormat::Unknown,
                None,
                vec![
                    "无法识别的格式：既不是 %PDF-、JSON、HTML/XML，也不是合法 UTF-8 纯文本"
                        .to_string(),
                ],
            ),
            "format-unknown",
            ErrorCode::Unsupported,
            elapsed(),
        ),
    }
}

/// DOC-09：把 HTML/XML 交给解析层，再把结果装进 DocText。
///
/// 语义：解析层失败 → 原样把它的 code/error 传出去（不吞错、不降级成成功）；
/// 解析层降级（如 Readability 回退）→ 保留其 degraded_from 与 code（禁止静默降级）。
fn markup_text(bytes: &[u8], format: DocFormat, elapsed_ms: u64) -> Envelope<DocText> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            return Envelope::fail(
                ErrorCode::Parse,
                format!("{format:?} 输入不是合法 UTF-8：{error}"),
                elapsed_ms,
            );
        }
    };

    if format == DocFormat::Html {
        let env = crate::parse::extract_article(text, None);
        let Some(article) = env.data else {
            return Envelope::fail(env.code, env.error, elapsed_ms);
        };
        return wrap_text(
            format,
            article.text,
            env.code,
            env.meta.degraded_from,
            elapsed_ms,
        );
    }

    let env = crate::parse::parse_xml(text);
    let Some(structured) = env.data else {
        return Envelope::fail(env.code, env.error, elapsed_ms);
    };
    let mut parts = Vec::new();
    collect_xml_text(&structured.value, &mut parts);
    wrap_text(
        format,
        parts.join("\n"),
        env.code,
        env.meta.degraded_from,
        elapsed_ms,
    )
}

fn wrap_text(
    format: DocFormat,
    text: String,
    upstream_code: ErrorCode,
    degraded_from: Option<String>,
    elapsed_ms: u64,
) -> Envelope<DocText> {
    let data = DocText {
        format,
        text: Some(text),
        pages: None,
        pdf_kind: None,
        diagnostics: Vec::new(),
    };
    match degraded_from {
        Some(from) => Envelope::degraded(data, from, upstream_code, elapsed_ms),
        None => Envelope::ok(data, Trust::UntrustedPageContent, elapsed_ms),
    }
}

/// 只收 XML 的**文本节点**：跳过 \`tag\`（标签名）与 \`attrs\`（属性值），
/// 避免把标记结构混进正文。
fn collect_xml_text(value: &Value, out: &mut Vec<String>) {
    let Value::Object(map) = value else {
        return;
    };
    for (key, child) in map {
        match key.as_str() {
            "tag" | "attrs" => {}
            "children" => {
                if let Value::Array(items) = child {
                    for item in items {
                        match item {
                            Value::String(text) => {
                                let trimmed = text.trim();
                                if !trimmed.is_empty() {
                                    out.push(trimmed.to_owned());
                                }
                            }
                            other => collect_xml_text(other, out),
                        }
                    }
                }
            }
            _ => collect_xml_text(child, out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 格式判定五类互斥。
    #[test]
    fn format_detection_is_deterministic() {
        assert_eq!(detect_format(b"%PDF-1.4\n"), DocFormat::Pdf);
        assert_eq!(detect_format(b"  {\"a\": 1}"), DocFormat::Json);
        assert_eq!(detect_format(b"[1,2,3]"), DocFormat::Json);
        assert_eq!(detect_format(b"<?xml version=\"1.0\"?>"), DocFormat::Xml);
        assert_eq!(
            detect_format(b"<!DOCTYPE html><html></html>"),
            DocFormat::Html
        );
        assert_eq!(
            detect_format(b"<html><body>x</body></html>"),
            DocFormat::Html
        );
        assert_eq!(detect_format(b"<rss><item/></rss>"), DocFormat::Xml);
        assert_eq!(detect_format(b"just some text\n"), DocFormat::Text);
        assert_eq!(detect_format(&[0x00, 0x01, 0xff]), DocFormat::Unknown);
    }

    /// DOC-11：未知格式 UNSUPPORTED，带可读诊断与 degraded_from。
    #[test]
    fn unknown_format_is_unsupported() {
        let env = doc_text(&[0x00, 0x01, 0x02, 0xff], &DocOptions::default());
        assert!(env.ok);
        assert_eq!(env.code, ErrorCode::Unsupported);
        assert_eq!(env.meta.degraded_from.as_deref(), Some("format-unknown"));
        assert_eq!(env.validate(), Ok(()));
        let data = env.data.as_ref().expect("data");
        assert!(data.text.is_none(), "不支持时不得返回文本");
        assert!(!data.diagnostics.is_empty());
    }

    /// DOC-10：超限 LIMIT，不解析。
    #[test]
    fn oversized_doc_is_limit() {
        let options = DocOptions { max_bytes: 8 };
        let env = doc_text(b"%PDF-1.4 and a lot more", &options);
        assert!(env.ok);
        assert_eq!(env.code, ErrorCode::Limit);
        assert_eq!(env.meta.degraded_from.as_deref(), Some("input-size-limit"));
        assert_eq!(env.validate(), Ok(()));
    }

    /// 纯文本原样返回。
    #[test]
    fn plain_text_roundtrips() {
        let env = doc_text("hello 世界\n".as_bytes(), &DocOptions::default());
        assert!(env.ok);
        assert_eq!(env.code, ErrorCode::Ok);
        let data = env.data.as_ref().expect("data");
        assert_eq!(data.text.as_deref(), Some("hello 世界\n"));
    }
}
