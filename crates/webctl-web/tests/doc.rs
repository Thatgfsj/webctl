//! DOC-01…DOC-11 的离线集成测试（PR 07）。
//!
//! 全部用例只读本目录 fixtures/doc 下的本地样本，不访问外网、不启动子进程。
//! fixture 生成方式见 doc 模块文档；PDF 样本为手写的最小组件：
//! - text.pdf：未压缩、BT/Tj/ET 的 Type1 Helvetica 文本页；
//! - chinese.pdf：Type0 / Identity-H + ToUnicode CMap（合成样本，非真实世界 PDF）；
//! - scanned.pdf：只有图片 XObject、无任何文本算子；
//! - encrypted.pdf：用 lopdf 对 text.pdf 做 RC4 40-bit 标准加密生成的真实加密文件；
//! - corrupt.pdf：声明 %PDF- 但结构损坏。

use webctl_core::{ErrorCode, Trust};
use webctl_web::{
    DocFormat, DocOptions, JsonQueryOptions, PdfKind, doc_text, extract_pdf_text, json_query,
};

const TEXT_PDF: &[u8] = include_bytes!("fixtures/doc/text.pdf");
const CHINESE_PDF: &[u8] = include_bytes!("fixtures/doc/chinese.pdf");
const SCANNED_PDF: &[u8] = include_bytes!("fixtures/doc/scanned.pdf");
const ENCRYPTED_PDF: &[u8] = include_bytes!("fixtures/doc/encrypted.pdf");
const CORRUPT_PDF: &[u8] = include_bytes!("fixtures/doc/corrupt.pdf");
const SAMPLE_JSON: &[u8] = include_bytes!("fixtures/doc/sample.json");
const SAMPLE_HTML: &[u8] = include_bytes!("fixtures/doc/sample.html");
const SAMPLE_XML: &[u8] = include_bytes!("fixtures/doc/sample.xml");
const SAMPLE_TXT: &[u8] = include_bytes!("fixtures/doc/sample.txt");
const UNKNOWN_BIN: &[u8] = include_bytes!("fixtures/doc/unknown.bin");

fn doc() -> DocOptions {
    DocOptions::default()
}

/// DOC-01：文本型 PDF 提取成功且关键词命中。
#[test]
fn doc_01_text_pdf_extracts_keywords() {
    let env = doc_text(TEXT_PDF, &doc());
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.code, ErrorCode::Ok);
    assert_eq!(env.meta.trust, Trust::UntrustedPageContent);
    assert_eq!(env.validate(), Ok(()));
    let data = env.data.as_ref().expect("data");
    assert_eq!(data.format, DocFormat::Pdf);
    assert_eq!(data.pdf_kind, Some(PdfKind::TextLayer));
    assert_eq!(data.pages, Some(1));
    let text = data.text.as_deref().expect("text");
    assert!(
        text.contains("Hello webctl PDF text layer"),
        "实际文本：{text:?}"
    );
}

/// DOC-02：中文 PDF 不乱码（合成 Type0 + ToUnicode 样本）。
#[test]
fn doc_02_chinese_pdf_is_not_mojibake() {
    let env = doc_text(CHINESE_PDF, &doc());
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.code, ErrorCode::Ok);
    assert_eq!(env.validate(), Ok(()));
    let text = env
        .data
        .as_ref()
        .expect("data")
        .text
        .as_deref()
        .expect("text");
    assert!(text.contains("你好"), "实际文本：{text:?}");
    assert!(text.contains("世界"), "实际文本：{text:?}");
    assert!(!text.contains('\u{fffd}'), "不得出现替换字符：{text:?}");
}

/// DOC-03：扫描件必须 UNSUPPORTED，不得返回空文本当成功。
#[test]
fn doc_03_scanned_pdf_is_unsupported() {
    let env = doc_text(SCANNED_PDF, &doc());
    assert!(env.ok);
    assert_eq!(env.code, ErrorCode::Unsupported);
    assert_eq!(env.meta.degraded_from.as_deref(), Some("pdf-no-text-layer"));
    assert_eq!(env.validate(), Ok(()));
    let data = env.data.as_ref().expect("data");
    assert_eq!(data.pdf_kind, Some(PdfKind::ScannedOrImageOnly));
    assert!(data.text.is_none(), "扫描件不得返回文本（哪怕空字符串）");
    assert!(!data.diagnostics.is_empty(), "必须带可读诊断");
    // 反面断言：不允许出现 code=OK 且 text 为空/纯空白的伪成功。
    assert!(
        !(env.code == ErrorCode::Ok
            && data.text.as_deref().map(str::trim).unwrap_or("").is_empty())
    );
}

/// DOC-04：加密 PDF 明确错误、不 hang、不崩。
#[test]
fn doc_04_encrypted_pdf_is_explicit_error() {
    let env = doc_text(ENCRYPTED_PDF, &doc());
    assert!(env.ok);
    assert_eq!(env.code, ErrorCode::Unsupported);
    assert_eq!(env.meta.degraded_from.as_deref(), Some("pdf-encrypted"));
    assert_eq!(env.validate(), Ok(()));
    let data = env.data.as_ref().expect("data");
    assert_eq!(data.pdf_kind, Some(PdfKind::Encrypted));
    assert!(data.text.is_none());
    assert!(
        data.diagnostics.iter().any(|d| d.contains("/Encrypt")),
        "诊断应说明加密：{:?}",
        data.diagnostics
    );
}

/// DOC-05：损坏 PDF 返回明确错误码。
#[test]
fn doc_05_corrupt_pdf_is_parse_error() {
    let env = doc_text(CORRUPT_PDF, &doc());
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Parse);
    assert!(!env.error.is_empty());
    assert!(env.data.is_none());
    assert_eq!(env.validate(), Ok(()));
}

/// DOC-06：json_query 路径取值。
#[test]
fn doc_06_json_path_selection() {
    let env = json_query(
        SAMPLE_JSON,
        "$.store.book[0].title",
        &JsonQueryOptions::default(),
    );
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.code, ErrorCode::Ok);
    assert_eq!(env.meta.trust, Trust::UntrustedPageContent);
    assert_eq!(env.validate(), Ok(()));
    let outcome = env.data.as_ref().expect("data");
    assert_eq!(outcome.value, serde_json::json!("Book A"));
    assert_eq!(outcome.matched, 1);
}

/// DOC-07：json_query 过滤与投影语义与文档一致。
#[test]
fn doc_07_json_filter_and_projection() {
    let filtered = json_query(
        SAMPLE_JSON,
        "$.store.book[?(@.price < 10)].title",
        &JsonQueryOptions::default(),
    );
    assert!(filtered.ok, "{}", filtered.error);
    assert_eq!(
        filtered.data.as_ref().expect("data").value,
        serde_json::json!(["Book A", "Book C"])
    );

    let projected = json_query(
        SAMPLE_JSON,
        "$.store.book[*].{title, price}",
        &JsonQueryOptions::default(),
    );
    assert!(projected.ok, "{}", projected.error);
    assert_eq!(
        projected.data.as_ref().expect("data").value,
        serde_json::json!([
            {"title": "Book A", "price": 8.5},
            {"title": "Book B", "price": 12.0},
            {"title": "Book C", "price": 5.0}
        ])
    );

    // 过滤零命中：空结果仍是成功，与解析失败严格区分。
    let empty = json_query(
        SAMPLE_JSON,
        "$.store.book[?(@.price > 100)]",
        &JsonQueryOptions::default(),
    );
    assert!(empty.ok);
    assert_eq!(empty.code, ErrorCode::Ok);
    assert_eq!(
        empty.data.as_ref().expect("data").value,
        serde_json::json!([])
    );
    assert_eq!(empty.data.as_ref().expect("data").matched, 0);
}

/// DOC-08：非法表达式明确错误，不返回 null 当成功。
#[test]
fn doc_08_invalid_expression_errors() {
    for bad in [
        "$.store.book[?(@.price <)]",
        "$.store.book[",
        "$.store.book[abc]",
        "$[?(@.price < 10)",
    ] {
        let env = json_query(SAMPLE_JSON, bad, &JsonQueryOptions::default());
        assert!(!env.ok, "{bad:?} 不应成功");
        assert_eq!(env.code, ErrorCode::Parse, "{bad:?}");
        assert!(!env.error.is_empty(), "{bad:?}");
        assert!(env.data.is_none(), "{bad:?}");
        assert_eq!(env.validate(), Ok(()));
    }
}

/// DOC-09（部分）：HTML/XML 到文本属 PR 03；本 PR 显式 UNSUPPORTED，不重复实现。
///
/// 这条用例锁定当前边界：等 PR 03 解析层合并后，本模块改为调用它，
/// 届时该用例应更新为与 PARSE-01 一致的结果。
#[test]
fn doc_09_html_and_xml_are_explicitly_deferred_to_pr03() {
    for (bytes, format) in [(SAMPLE_HTML, DocFormat::Html), (SAMPLE_XML, DocFormat::Xml)] {
        let env = doc_text(bytes, &doc());
        assert!(env.ok);
        assert_eq!(env.code, ErrorCode::Unsupported);
        assert_eq!(
            env.meta.degraded_from.as_deref(),
            Some("parse-layer-pending-pr03")
        );
        assert_eq!(env.validate(), Ok(()));
        let data = env.data.as_ref().expect("data");
        assert_eq!(data.format, format);
        assert!(data.text.is_none());
        assert!(!data.diagnostics.is_empty());
    }

    // 纯文本走直通分支，证明本模块没有重复造解析层。
    let text = doc_text(SAMPLE_TXT, &doc());
    assert!(text.ok);
    assert_eq!(text.code, ErrorCode::Ok);
    assert_eq!(
        text.data.as_ref().expect("data").text.as_deref(),
        Some("plain text layer\nsecond line\n")
    );

    // JSON 交给 json_query，不在 doc_text 里重复解析。
    let json = doc_text(SAMPLE_JSON, &doc());
    assert_eq!(json.code, ErrorCode::Unsupported);
    assert_eq!(json.meta.degraded_from.as_deref(), Some("use-json-query"));
}

/// DOC-10：大输入命中上限返回 LIMIT，不解析、不 OOM。
#[test]
fn doc_10_oversized_input_is_limit() {
    let mut huge = b"%PDF-1.4\n".to_vec();
    huge.extend(vec![b'A'; 9 * 1024 * 1024]);
    let env = doc_text(&huge, &doc());
    assert!(env.ok);
    assert_eq!(env.code, ErrorCode::Limit);
    assert_eq!(env.meta.degraded_from.as_deref(), Some("input-size-limit"));
    assert_eq!(env.validate(), Ok(()));
    assert!(env.data.as_ref().expect("data").text.is_none());

    // 直接调用 PDF 层，使用更小的上限，仍走 LIMIT。
    let env = extract_pdf_text(TEXT_PDF, 8);
    assert_eq!(env.code, ErrorCode::Limit);
    assert_eq!(
        env.meta.degraded_from.as_deref(),
        Some("pdf-input-size-limit")
    );

    // JSON 大输入的对应保护。
    let big_json = format!("{{\"pad\": \"{}\"}}", "A".repeat(2 * 1024 * 1024));
    let env = json_query(
        big_json.as_bytes(),
        "$.pad",
        &JsonQueryOptions { max_bytes: 1024 },
    );
    assert_eq!(env.code, ErrorCode::Limit);
    assert_eq!(
        env.meta.degraded_from.as_deref(),
        Some("json-input-size-limit")
    );
}

/// DOC-11：未知格式 UNSUPPORTED + 可读诊断。
#[test]
fn doc_11_unknown_format_is_unsupported_with_diagnostic() {
    let env = doc_text(UNKNOWN_BIN, &doc());
    assert!(env.ok);
    assert_eq!(env.code, ErrorCode::Unsupported);
    assert_eq!(env.meta.degraded_from.as_deref(), Some("format-unknown"));
    assert_eq!(env.validate(), Ok(()));
    let data = env.data.as_ref().expect("data");
    assert_eq!(data.format, DocFormat::Unknown);
    assert!(data.text.is_none());
    assert!(!data.diagnostics.is_empty());
    assert!(data.diagnostics[0].chars().count() >= 8, "诊断应可读");
}

/// 四类结局码互不相同：空结果 / 解析失败 / 能力不支持 /（本模块不产生 BLOCKED）。
#[test]
fn four_outcomes_have_distinct_codes() {
    assert_eq!(doc_text(TEXT_PDF, &doc()).code, ErrorCode::Ok);
    assert_eq!(doc_text(CORRUPT_PDF, &doc()).code, ErrorCode::Parse);
    assert_eq!(doc_text(SCANNED_PDF, &doc()).code, ErrorCode::Unsupported);
    let empty = json_query(
        SAMPLE_JSON,
        "$.store.book[?(@.price > 100)]",
        &JsonQueryOptions::default(),
    );
    assert_eq!(empty.code, ErrorCode::Ok, "空结果仍是成功码");
    assert_eq!(empty.data.expect("data").matched, 0);
}
