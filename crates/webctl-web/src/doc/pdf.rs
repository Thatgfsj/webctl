//! PDF 文本层提取（PLAN v0.4 §2「文档/数据」域，PR 07；替代外部 pdftotext）。
//!
//! # 前置 spike 结论（PLAN §9 待验证项 2）
//!
//! 在本机（Rust 1.98.1，无系统 libclang，仅 BoringSSL bindgen 需要它）实测：
//!
//! - pdf-extract 0.12.1 能编译并提取文本；对一份手写的最小文本型 PDF 得到
//!   "Hello webctl PDF text layer"。
//! - 对一份手写的 Type0 / Identity-H + ToUnicode CMap 最小中文 PDF，pdf-extract
//!   得到 "你好"（未嵌字体，仅 CMap 路径）。
//! - 对图片型（无文本算子）PDF，pdf-extract 返回 Ok("")——本层必须把它转成
//!   UNSUPPORTED，绝不能把空字符串当成功。
//! - **panic 触发条件（实测，订正早期描述）**：图片型 PDF 走 Ok("")，**不是**
//!   panic 的触发条件；真正会 panic 的是 pdf-extract 0.12.1 未建模的正文算子，
//!   例如 `Tj` 的操作数不是字符串（其 src/lib.rs 的 "Tj" 分支直接
//!   `panic!("unexpected Tj operand")`），或 `Tf` 引用了 /Font 里不存在的字体名
//!   （其 get(...) 会 expect 该键存在）。本层用 catch_unwind 把这类 panic 隔离为
//!   PARSE；离线样本见 tests/fixtures/doc/panic_tj.pdf。
//! - lopdf 0.42.0 正是 pdf-extract 自己的依赖；直接依赖它不引入重复版本
//!   （cargo tree 验证只有一份 lopdf 0.42.0），用于结构预检。
//! - 真实世界 PDF、内嵌字体子集、复杂 CJK 编码未验证（见 PR 正文缺口）。
//!
//! # 处理顺序（保证各类失败可区分）
//!
//! 1. lopdf 载入失败 -> PARSE（损坏）；
//! 2. lopdf 判定 /Encrypt -> UNSUPPORTED（不尝试解密、不 hang）；
//! 3. 页数为 0 -> UNSUPPORTED；
//! 4. pdf-extract 提取：Ok 且非空 -> OK；Ok 但纯空白 -> UNSUPPORTED（扫描件/无文本层）；
//!    Err -> PARSE；
//! 5. pdf-extract 内部 panic -> catch_unwind 隔离为 PARSE（进程不崩）。

use std::time::Instant;

use webctl_core::{Envelope, ErrorCode, Trust};

use crate::doc::{DocFormat, DocText, PdfKind};

/// 提取 PDF 文本层。
///
/// max_bytes 是本层自己的兜底上限；doc_text 路由已在更外层检查过。
pub fn extract_pdf_text(bytes: &[u8], max_bytes: usize) -> Envelope<DocText> {
    let started = Instant::now();
    let elapsed = || started.elapsed().as_millis() as u64;

    if bytes.len() > max_bytes {
        return Envelope::degraded(
            DocText::unsupported(
                DocFormat::Pdf,
                Some(PdfKind::Malformed),
                vec![format!(
                    "PDF 输入 {} 字节，超过上限 {max_bytes} 字节，未解析",
                    bytes.len()
                )],
            ),
            "pdf-input-size-limit",
            ErrorCode::Limit,
            elapsed(),
        );
    }

    let document = match lopdf::Document::load_mem(bytes) {
        Ok(document) => document,
        Err(error) => {
            return Envelope::fail(
                ErrorCode::Parse,
                format!("PDF 结构损坏，无法解析：{error}"),
                elapsed(),
            );
        }
    };

    if document.is_encrypted() {
        return Envelope::degraded(
            DocText::unsupported(
                DocFormat::Pdf,
                Some(PdfKind::Encrypted),
                vec!["PDF 带 /Encrypt，本实现不提供解密能力；未尝试提取文本".to_string()],
            ),
            "pdf-encrypted",
            ErrorCode::Unsupported,
            elapsed(),
        );
    }

    let pages = document.get_pages().len();
    if pages == 0 {
        return Envelope::degraded(
            DocText::unsupported(
                DocFormat::Pdf,
                Some(PdfKind::NoPages),
                vec!["PDF 没有任何页面对象".to_string()],
            ),
            "pdf-no-pages",
            ErrorCode::Unsupported,
            elapsed(),
        );
    }

    // pdf-extract 0.12.1 对未建模的正文算子会 panic：实测 `Tj` 的操作数不是
    // 字符串时，其 src/lib.rs 的 "Tj" 分支直接 panic!("unexpected Tj operand")；
    // 而「无滤镜图片 XObject」并不会 panic（它走 Ok("") -> pdf-no-text-layer）。
    // 这里用 catch_unwind 把真实的解析库 panic 隔离为 PARSE，保证 doc_text
    // 绝不因解析库崩溃而中断。用例：tests/doc.rs::doc_pdf_extract_panic_is_isolated_as_parse。
    let extracted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem(bytes)
    }));
    match extracted {
        Ok(Ok(text)) => {
            if text.trim().is_empty() {
                Envelope::degraded(
                    DocText::unsupported(
                        DocFormat::Pdf,
                        Some(PdfKind::ScannedOrImageOnly),
                        vec![
                            "PDF 没有可提取的文本层（疑似扫描件或纯图片页）；OCR 不在 v0.4 能力范围"
                                .to_string(),
                        ],
                    ),
                    "pdf-no-text-layer",
                    ErrorCode::Unsupported,
                    elapsed(),
                )
            } else {
                Envelope::ok(
                    DocText {
                        format: DocFormat::Pdf,
                        text: Some(text),
                        pages: Some(pages),
                        pdf_kind: Some(PdfKind::TextLayer),
                        diagnostics: Vec::new(),
                    },
                    Trust::UntrustedPageContent,
                    elapsed(),
                )
            }
        }
        Ok(Err(error)) => Envelope::fail(
            ErrorCode::Parse,
            format!("PDF 文本层提取失败：{error}"),
            elapsed(),
        ),
        Err(_) => Envelope::fail(
            ErrorCode::Parse,
            "PDF 文本层提取时解析库内部异常（已隔离）；文件结构可能不受支持".to_string(),
            elapsed(),
        ),
    }
}
