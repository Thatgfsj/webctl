//! 解析层（PLAN v0.4 §2「解析」域，PR 03）。
//!
//! 目标：把 PR 02 取回的文本变成 AI 能直接用的结构——正文、链接、表格、表单、
//! JSON/XML、markdown 与纯文本两种视图。
//!
//! 硬约束：
//! - 所有公开返回走 webctl_core::Envelope；页面内容固定
//!   webctl_core::Trust::UntrustedPageContent。
//! - 空结果（零命中）与解析失败严格区分：前者 ok=true + 空数组，后者
//!   ok=false + code=PARSE。
//! - 非 HTML 输入不硬解析：路由到对应处理器；`text/*` 原样按纯文本返回
//!   （`Parsed::Text`，OK 且可回溯，不是 UNSUPPORTED、也不静默降级）；
//!   真正非文本的 pdf/binary 才显式 UNSUPPORTED。
//! - 回退路径是**显式降级**：code=DEGRADED 且 meta.degraded_from 非空。
//! - 零子进程、零 unsafe（workspace 级 unsafe_code = "forbid"）。

use std::time::Instant;

use serde::{Deserialize, Serialize};
use webctl_core::{Envelope, ErrorCode, Trust};

pub mod article;
pub mod forms;
pub mod html;
pub mod links;
pub mod structured;
pub mod tables;

pub use article::{Article, extract_article, extract_article_fallback};
pub use forms::{Form, FormField, FormOption, extract_forms};
pub use links::{Link, extract_links};
pub use structured::{Parsed, PlainText, Structured, parse_body, parse_json, parse_xml};
pub use tables::{Table, TableCell, extract_tables};

/// CSS 选择器的一次命中。结构化路径与上下文由 PR 06 的 extract 补齐；
/// 本 PR 只保证命中**保序**（与文档序一致）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectorMatch {
    /// 命中序号（0 起，按文档序）。
    pub index: usize,
    /// 元素标签名（小写）。
    pub tag: String,
    /// 该元素所有后代文本折叠空白后的结果。
    pub text: String,
    /// 该元素的 outer HTML。
    pub html: String,
}

/// 用 CSS 选择器抽取元素，命中按文档序返回。
///
/// 语义边界（与 EXTRACT-06 / ENV-10 对齐）：
/// - 合法选择器 + 零命中 → ok=true，data 为空数组；
/// - 非法/空选择器 → ok=false，code=PARSE。
pub fn select(html: &str, selector: &str) -> Envelope<Vec<SelectorMatch>> {
    let started = Instant::now();
    let css = match html::compile_selector(selector) {
        Ok(css) => css,
        Err(message) => return Envelope::fail(ErrorCode::Parse, message, elapsed_ms(started)),
    };
    let document = html::parse_document(html);
    let matches = document
        .select(&css)
        .enumerate()
        .map(|(index, element)| SelectorMatch {
            index,
            tag: element.value().name().to_ascii_lowercase(),
            text: html::collapse_ws(&element.text().collect::<String>()),
            html: element.html(),
        })
        .collect();
    Envelope::ok(matches, Trust::UntrustedPageContent, elapsed_ms(started))
}

/// 实测耗时（毫秒）。所有公开入口用它填充 meta.elapsed_ms。
pub(crate) fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}
