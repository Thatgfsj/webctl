//! 表格 / 表单 / 正文路由（EXTRACT-10、11、15、16；FORM-09）。
//!
//! 解析与结构完全复用 crate::parse 的 extract_tables / extract_forms，本模块只
//! 叠加：命中上限截断（LIMIT）、form_index 选择（FORM-09）、以及按内容类型的
//! 路由——CSS 选择器只对 HTML 有意义，pdf/binary/text/json/xml 走 UNSUPPORTED
//! 的显式降级（不是失败，也不是静默空结果）。

use std::time::Instant;

use webctl_core::{Envelope, ErrorCode, Trust};

use crate::content_type::ContentKind;
use crate::parse::elapsed_ms;
use crate::parse::{Form, Table, extract_forms, extract_tables};
use crate::transport::Body;

use super::{ExtractOptions, ExtractResult, select_matches};

/// 抽取表格；结构与 crate::parse::extract_tables 完全一致（EXTRACT-10、15）。
///
/// 超过 max_results 时截断并返回 code=LIMIT + 非空 degraded_from。
pub fn tables(html: &str, options: &ExtractOptions) -> Envelope<Vec<Table>> {
    let started = Instant::now();
    let extracted = extract_tables(html);
    if !extracted.ok {
        return Envelope::fail(extracted.code, extracted.error, elapsed_ms(started));
    }
    let already = extracted.meta.degraded_from.clone();
    let mut data = extracted.data.unwrap_or_default();
    let truncated = data.len() > options.max_results;
    if truncated {
        data.truncate(options.max_results);
    }
    degraded_or_ok(data, already, truncated, started)
}

/// 抽取表单；结构与 crate::parse::extract_forms 完全一致（EXTRACT-11、15；FORM-09）。
pub fn forms(html: &str, base: Option<&str>, options: &ExtractOptions) -> Envelope<Vec<Form>> {
    let started = Instant::now();
    let extracted = extract_forms(html, base);
    if !extracted.ok {
        return Envelope::fail(extracted.code, extracted.error, elapsed_ms(started));
    }
    let already = extracted.meta.degraded_from.clone();
    let mut data = extracted.data.unwrap_or_default();
    let truncated = data.len() > options.max_results;
    if truncated {
        data.truncate(options.max_results);
    }
    degraded_or_ok(data, already, truncated, started)
}

/// 按 form_index 选定一个表单（FORM-09）。
///
/// 越界是请求错误：ok=false + code=PARSE + 明确说明页面有几个表单，
/// 不返回空数组、不静默取第一个。
pub fn form_by_index(html: &str, base: Option<&str>, form_index: usize) -> Envelope<Form> {
    let started = Instant::now();
    let extracted = extract_forms(html, base);
    if !extracted.ok {
        return Envelope::fail(extracted.code, extracted.error, elapsed_ms(started));
    }
    let forms = extracted.data.unwrap_or_default();
    let count = forms.len();
    match forms.into_iter().nth(form_index) {
        Some(form) => Envelope::ok(form, Trust::UntrustedPageContent, elapsed_ms(started)),
        None => Envelope::fail(
            ErrorCode::Parse,
            format!("form_index {form_index} out of range: page has {count} form(s)"),
            elapsed_ms(started),
        ),
    }
}

/// 按内容类型抽取：HTML 走选择器；其它类型显式 UNSUPPORTED（EXTRACT-16 由此
/// 保证二进制内容不会被塞进结果）。
pub fn extract_body(
    body: &Body,
    selector: &str,
    options: &ExtractOptions,
) -> Envelope<ExtractResult> {
    let started = Instant::now();
    match body.kind {
        ContentKind::Html => match body.text.as_deref() {
            Some(text) => select_matches(text, selector, options),
            None => Envelope::fail(
                ErrorCode::Internal,
                "content-type routing expected decoded text but body.text is None",
                elapsed_ms(started),
            ),
        },
        other => Envelope::degraded(
            ExtractResult::unsupported(
                selector,
                format!(
                    "CSS selector extraction requires HTML; content kind is {} — \
                     route it to search() for text or to the document handler",
                    other.as_str()
                ),
            ),
            format!("content-type:{}", other.as_str()),
            ErrorCode::Unsupported,
            elapsed_ms(started),
        ),
    }
}

fn degraded_or_ok<T>(
    data: Vec<T>,
    already: Option<String>,
    truncated: bool,
    started: Instant,
) -> Envelope<Vec<T>> {
    match (already, truncated) {
        (None, false) => Envelope::ok(data, Trust::UntrustedPageContent, elapsed_ms(started)),
        (Some(from), false) => {
            Envelope::degraded(data, from, ErrorCode::Limit, elapsed_ms(started))
        }
        (None, true) => {
            Envelope::degraded(data, "max-results", ErrorCode::Limit, elapsed_ms(started))
        }
        (Some(from), true) => Envelope::degraded(
            data,
            format!("{from},max-results"),
            ErrorCode::Limit,
            elapsed_ms(started),
        ),
    }
}
