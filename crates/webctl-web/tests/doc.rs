//! DOC-01…DOC-11 的离线集成测试（PR 07）。
//!
//! 全部用例只读本目录 fixtures/doc 下的本地样本，不访问外网、不启动子进程。
//! fixture 生成方式见 doc 模块文档；PDF 样本为手写的最小组件：
//! - text.pdf：未压缩、BT/Tj/ET 的 Type1 Helvetica 文本页；
//! - chinese.pdf：Type0 / Identity-H + ToUnicode CMap（合成样本，非真实世界 PDF）；
//! - scanned.pdf：只有图片 XObject、无任何文本算子；
//! - encrypted.pdf：用 lopdf 对 text.pdf 做 RC4 40-bit 标准加密生成的真实加密文件；
//! - corrupt.pdf：声明 %PDF- 但结构损坏；
//! - panic_tj.pdf：正文算子是 `123 Tj`（Tj 的操作数不是字符串），
//!   触发 pdf-extract 0.12.1 内部 panic，用来覆盖 catch_unwind 隔离分支。

use webctl_core::{ErrorCode, Trust};
use webctl_web::{
    DocFormat, DocOptions, JsonQueryOptions, PdfKind, doc_text, extract_pdf_text, json_query,
};

const TEXT_PDF: &[u8] = include_bytes!("fixtures/doc/text.pdf");
const CHINESE_PDF: &[u8] = include_bytes!("fixtures/doc/chinese.pdf");
const SCANNED_PDF: &[u8] = include_bytes!("fixtures/doc/scanned.pdf");
const ENCRYPTED_PDF: &[u8] = include_bytes!("fixtures/doc/encrypted.pdf");
const CORRUPT_PDF: &[u8] = include_bytes!("fixtures/doc/corrupt.pdf");
/// 触发 pdf-extract 内部 panic 的最小 PDF（`123 Tj`：Tj 操作数不是字符串）。
const PANIC_PDF: &[u8] = include_bytes!("fixtures/doc/panic_tj.pdf");
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

/// catch_unwind 覆盖：pdf-extract 内部 panic 必须归为 PARSE，不得崩溃或伪装成功。
///
/// 触发点（实测 pdf-extract 0.12.1 `src/lib.rs`）：正文流 `123 Tj` 的操作数不是
/// 字符串，其 `"Tj"` 分支直接 `panic!("unexpected Tj operand")`。doc_text 内部的
/// catch_unwind 必须把它隔离为 PARSE。这里在测试外再包一层 catch_unwind，确保
/// 隔离一旦回退，本用例以明确信息失败而不是直接炸掉测试进程；同时临时静音
/// panic hook，避免预期内的 panic 噪声污染 `cargo test` 日志。
#[test]
fn doc_pdf_extract_panic_is_isolated_as_parse() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(|| doc_text(PANIC_PDF, &doc()));
    std::panic::set_hook(hook);

    let env = outcome.expect("catch_unwind 必须隔离 pdf-extract 的 panic");
    assert!(!env.ok, "panic 不得伪装成成功");
    assert_eq!(env.code, ErrorCode::Parse);
    assert!(env.data.is_none(), "PARSE 不得返回 data");
    assert!(!env.error.is_empty(), "PARSE 必须带可读错误");
    // 必须是 catch_unwind 的隔离分支，而不是 lopdf 载入失败/其它 PARSE：
    // 若将来 pdf-extract 不再 panic，本断言会失败，提醒更新触发样本而不是静默退化。
    assert!(
        env.error.contains("已隔离"),
        "应命中 catch_unwind 隔离分支，实际错误：{}",
        env.error
    );
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

/// DOC-09：HTML/XML→文本复用解析层（PR 03）后的**正式验收**。
///
/// PR 03 合并（`5afc240`）后本用例已从 `#[ignore]` 转正：doc_text 对 HTML/XML 必须成功、
/// 无降级标识、正文非空且已剥离标签（与 PARSE-01 同一份正文），不再有 `UNSUPPORTED` 假装。
#[test]
fn doc_09_acceptance_after_pr03() {
    for (bytes, format) in [(SAMPLE_HTML, DocFormat::Html), (SAMPLE_XML, DocFormat::Xml)] {
        let env = doc_text(bytes, &doc());
        assert!(env.ok);
        assert_eq!(
            env.code,
            ErrorCode::Ok,
            "{format:?} PR 03 合并后应成功，而非降级"
        );
        assert_eq!(
            env.meta.degraded_from, None,
            "{format:?} PR 03 合并后不应再有降级标识"
        );
        assert_eq!(env.validate(), Ok(()));
        let data = env.data.as_ref().expect("data");
        assert_eq!(data.format, format);
        let text = data.text.as_deref().expect("text");
        assert!(!text.is_empty(), "{format:?} 正文不应为空");
        // 与 PARSE-01 一致：只要正文，标签不应出现。
        assert!(!text.contains('<'), "{format:?} 应已剥离标签：{text:?}");
    }

    // 纯文本仍走直通分支（不解析、不降级）：证明本模块没有重复造解析层。
    // 这条断言原先在过渡期用例里，删掉那条用例时移到这里，覆盖不减。
    let text = doc_text(SAMPLE_TXT, &doc());
    assert!(text.ok);
    assert_eq!(text.code, ErrorCode::Ok);
    assert!(text.meta.degraded_from.is_none());
    assert!(text.data.as_ref().expect("data").text.is_some());
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
