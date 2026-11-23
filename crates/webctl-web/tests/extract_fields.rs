//! `extract.fields`（议题 #193）的离线验收：schema 抽字段的语义边界。
//!
//! 覆盖：h1 文本 / `all` 多值 / img src / 缺失字段 / 非法 selector / 零命中 /
//! 全部非法 / 全部被守卫拒绝 / 截断降级 / 非 HTML 路由 / 值序列化 /
//! 与 `select_matches` 的一致性（既有输出不变）。
//!
//! 全部用例只读内存 HTML，不联网、不启动子进程（与 `tests/extract.rs` 同口径）。

use std::collections::BTreeMap;

use webctl_core::{Envelope, ErrorCode, Trust};
use webctl_web::extract::{
    ExtractOptions, FieldDiagnostic, FieldIssueKind, FieldSetResult, FieldSpec, FieldValue,
    MAX_FIELDS, STABILITY_NOTE, extract_fields, extract_fields_body, select_matches,
};
use webctl_web::{Body, ContentKind};

const PAGE: &str = include_str!("fixtures/extract/page.html");

/// 两个 h1：验证 `all=false` 只取第一个，且文本折叠空白。
const HEADINGS: &str = "<html><body><h1>  Hello   World </h1><h1>Second</h1></body></html>";

/// 三张图：前两张在容器里、第三张在 body 下；第二张缺 `alt`。
const IMAGES: &str = "<html><body><div id=\"g\"><img src=\"/a.png\" alt=\"A\"><img src=\"/b.png\"></div><img src=\"/c.png\"></body></html>";

fn text(selector: &str) -> FieldSpec {
    FieldSpec {
        selector: selector.to_owned(),
        attr: None,
        all: false,
    }
}

fn all(selector: &str) -> FieldSpec {
    FieldSpec {
        all: true,
        ..text(selector)
    }
}

fn attr(selector: &str, name: &str) -> FieldSpec {
    FieldSpec {
        attr: Some(name.to_owned()),
        ..text(selector)
    }
}

fn attr_all(selector: &str, name: &str) -> FieldSpec {
    FieldSpec {
        all: true,
        ..attr(selector, name)
    }
}

fn specs(pairs: Vec<(&str, FieldSpec)>) -> BTreeMap<String, FieldSpec> {
    pairs
        .into_iter()
        .map(|(name, spec)| (name.to_owned(), spec))
        .collect()
}

fn options() -> ExtractOptions {
    ExtractOptions::default()
}

fn assert_valid<T: std::fmt::Debug>(envelope: &Envelope<T>) {
    assert_eq!(envelope.validate(), Ok(()), "信封不变量：{envelope:?}");
}

fn kinds(diagnostics: &[FieldDiagnostic]) -> Vec<(&str, FieldIssueKind)> {
    diagnostics
        .iter()
        .map(|entry| (entry.field.as_str(), entry.kind))
        .collect()
}

fn field<'a>(result: &'a FieldSetResult, name: &str) -> &'a FieldValue {
    result
        .fields
        .get(name)
        .unwrap_or_else(|| panic!("字段 {name} 必须出现（缺失也要给 null）：{result:?}"))
}

fn binary_body(kind: ContentKind) -> Body {
    Body {
        kind,
        content_type: Some("application/pdf".to_owned()),
        byte_len: 8,
        declared_len: None,
        truncated: false,
        text: None,
        charset: None,
        charset_source: None,
        decode_had_errors: false,
    }
}

/// FIELD-01：`attr` 缺省取元素文本（折叠空白），`all=false` 只取第一个命中。
#[test]
fn fields_01_h1_text_is_collapsed_and_first_match_only() {
    let declaration = specs(vec![("title", text("h1"))]);
    let envelope = extract_fields(HEADINGS, &declaration, &options());
    assert_valid(&envelope);
    assert!(envelope.ok, "{}", envelope.error);
    assert_eq!(envelope.code, ErrorCode::Ok);
    assert_eq!(envelope.meta.trust, Trust::UntrustedPageContent);
    let result = envelope.data.expect("fields data");
    assert_eq!(
        field(&result, "title"),
        &FieldValue::Text("Hello World".to_owned())
    );
    assert!(
        result.diagnostics.is_empty(),
        "全部成功时不应有诊断：{:?}",
        result.diagnostics
    );
}

/// FIELD-02：`all=true` 返回按文档序的数组，不留诊断。
#[test]
fn fields_02_all_returns_document_order_array() {
    let declaration = specs(vec![("items", all("li.item"))]);
    let envelope = extract_fields(PAGE, &declaration, &options());
    assert_valid(&envelope);
    assert!(envelope.ok, "{}", envelope.error);
    let result = envelope.data.expect("fields data");
    assert_eq!(
        field(&result, "items"),
        &FieldValue::List(vec![
            Some("ITEM-ONE".to_owned()),
            Some("ITEM-TWO".to_owned()),
            Some("ITEM-THREE".to_owned()),
        ])
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

/// FIELD-03：`attr = "src"` 取属性原值（不做空白折叠、不加绝对化）。
#[test]
fn fields_03_img_src_attribute() {
    let declaration = specs(vec![
        ("first_src", attr("img", "src")),
        ("all_src", attr_all("img", "src")),
    ]);
    let envelope = extract_fields(IMAGES, &declaration, &options());
    assert_valid(&envelope);
    assert!(envelope.ok, "{}", envelope.error);
    let result = envelope.data.expect("fields data");
    assert_eq!(
        field(&result, "first_src"),
        &FieldValue::Text("/a.png".to_owned())
    );
    assert_eq!(
        field(&result, "all_src"),
        &FieldValue::List(vec![
            Some("/a.png".to_owned()),
            Some("/b.png".to_owned()),
            Some("/c.png".to_owned()),
        ])
    );
}

/// FIELD-04：合法选择器零命中 -> 值 null + `zero-matches` 诊断，整体仍是成功。
#[test]
fn fields_04_zero_hits_is_null_with_diagnostic() {
    let declaration = specs(vec![("missing", text("article.nope"))]);
    let envelope = extract_fields(PAGE, &declaration, &options());
    assert_valid(&envelope);
    assert!(
        envelope.ok,
        "零命中是合法空结果，不是失败：{}",
        envelope.error
    );
    assert_eq!(envelope.code, ErrorCode::Ok);
    assert_eq!(envelope.meta.degraded_from, None::<String>);
    let result = envelope.data.expect("fields data");
    assert_eq!(field(&result, "missing"), &FieldValue::Null);
    assert_eq!(
        kinds(&result.diagnostics),
        vec![("missing", FieldIssueKind::ZeroMatches)]
    );
    assert!(result.conclusion.contains("diagnostic"));
}

/// FIELD-05：`all=true` 的零命中是空数组（不是 null），语义与 FIELD-04 区分。
#[test]
fn fields_05_zero_hits_all_is_empty_array() {
    let declaration = specs(vec![("missing", all("#nope"))]);
    let envelope = extract_fields(PAGE, &declaration, &options());
    assert_valid(&envelope);
    assert!(envelope.ok, "{}", envelope.error);
    assert_eq!(envelope.code, ErrorCode::Ok);
    let result = envelope.data.expect("fields data");
    assert_eq!(field(&result, "missing"), &FieldValue::List(Vec::new()));
    assert_eq!(
        kinds(&result.diagnostics),
        vec![("missing", FieldIssueKind::ZeroMatches)]
    );
}

/// FIELD-06：单个字段选择器非法 -> 只影响该字段（null + 诊断），其余字段照常取值。
#[test]
fn fields_06_invalid_selector_is_field_diagnostic() {
    let declaration = specs(vec![("title", text("h1")), ("broken", text("div["))]);
    let envelope = extract_fields(HEADINGS, &declaration, &options());
    assert_valid(&envelope);
    assert!(
        envelope.ok,
        "单字段非法不应升级为整体失败：{}",
        envelope.error
    );
    let result = envelope.data.expect("fields data");
    assert_eq!(
        field(&result, "title"),
        &FieldValue::Text("Hello World".to_owned())
    );
    assert_eq!(field(&result, "broken"), &FieldValue::Null);
    assert_eq!(
        kinds(&result.diagnostics),
        vec![("broken", FieldIssueKind::InvalidSelector)]
    );
    assert!(!result.diagnostics[0].message.is_empty());
}

/// FIELD-07：全部字段都非法 -> 整体 PARSE（不返回「成功但全 null」）。
#[test]
fn fields_07_all_invalid_is_parse_failure() {
    let declaration = specs(vec![("a", text("div[")), ("b", text("span["))]);
    let envelope = extract_fields(PAGE, &declaration, &options());
    assert_valid(&envelope);
    assert!(!envelope.ok);
    assert_eq!(envelope.code, ErrorCode::Parse);
    assert!(envelope.data.is_none());
    assert!(envelope.error.contains("全部非法"), "{}", envelope.error);
}

/// FIELD-08：空 `fields` 对象 -> PARSE（空 schema 是调用错误，不是空结果）。
#[test]
fn fields_08_empty_declaration_is_parse_failure() {
    let envelope = extract_fields(PAGE, &BTreeMap::new(), &options());
    assert_valid(&envelope);
    assert!(!envelope.ok);
    assert_eq!(envelope.code, ErrorCode::Parse);
    assert!(
        envelope.error.contains("不能为空对象"),
        "{}",
        envelope.error
    );
}

/// FIELD-09：字段数超过 MAX_FIELDS -> PARSE（防单次调用扫描无界）。
#[test]
fn fields_09_too_many_fields_is_parse_failure() {
    let declaration: BTreeMap<String, FieldSpec> = (0..=MAX_FIELDS)
        .map(|index| (format!("field_{index}"), text("h1")))
        .collect();
    let envelope = extract_fields(HEADINGS, &declaration, &options());
    assert_valid(&envelope);
    assert!(!envelope.ok);
    assert_eq!(envelope.code, ErrorCode::Parse);
    assert!(envelope.error.contains("最多声明"), "{}", envelope.error);
}

/// FIELD-10：字段名 / `attr` 非法 -> 逐字段诊断，不中断其余字段。
#[test]
fn fields_10_invalid_name_and_attribute_diagnostics() {
    let declaration = specs(vec![
        ("1bad", text("h1")),
        ("empty_attr", attr("h1", "   ")),
        ("title", text("h1")),
    ]);
    let envelope = extract_fields(HEADINGS, &declaration, &options());
    assert_valid(&envelope);
    assert!(envelope.ok, "{}", envelope.error);
    let result = envelope.data.expect("fields data");
    assert_eq!(field(&result, "1bad"), &FieldValue::Null);
    assert_eq!(field(&result, "empty_attr"), &FieldValue::Null);
    assert_eq!(
        field(&result, "title"),
        &FieldValue::Text("Hello World".to_owned())
    );
    assert_eq!(
        kinds(&result.diagnostics),
        vec![
            ("1bad", FieldIssueKind::InvalidName),
            ("empty_attr", FieldIssueKind::InvalidAttribute),
        ]
    );
}

/// FIELD-11：命中元素缺属性 -> 数组里保留 null 位置，并给出 `missing-attribute` 诊断。
#[test]
fn fields_11_missing_attribute_keeps_null_slots() {
    let declaration = specs(vec![("alts", attr_all("img", "alt"))]);
    let envelope = extract_fields(IMAGES, &declaration, &options());
    assert_valid(&envelope);
    assert!(envelope.ok, "{}", envelope.error);
    let result = envelope.data.expect("fields data");
    assert_eq!(
        field(&result, "alts"),
        &FieldValue::List(vec![Some("A".to_owned()), None, None])
    );
    assert_eq!(
        kinds(&result.diagnostics),
        vec![("alts", FieldIssueKind::MissingAttribute)]
    );
    assert!(
        result.diagnostics[0].message.contains('2'),
        "诊断要给出缺失个数：{}",
        result.diagnostics[0].message
    );
}

/// FIELD-12：`all=true` 被 `max_results` 截断 -> 整体 LIMIT 降级（显式，不静默）。
#[test]
fn fields_12_truncation_is_limit_degradation() {
    let mut opts = options();
    opts.max_results = 2;
    let declaration = specs(vec![("items", all("li.item"))]);
    let envelope = extract_fields(PAGE, &declaration, &opts);
    assert_valid(&envelope);
    assert!(envelope.ok, "截断是降级不是失败：{}", envelope.error);
    assert_eq!(envelope.code, ErrorCode::Limit);
    assert_eq!(
        envelope.meta.degraded_from.as_deref(),
        Some("fields-truncated")
    );
    let actionable = envelope
        .meta
        .actionable
        .as_ref()
        .expect("降级必须带 actionable");
    assert_eq!(actionable.reason, "field-limit-reached");
    assert!(!actionable.next_steps.is_empty());
    let result = envelope.data.expect("fields data");
    assert_eq!(
        field(&result, "items"),
        &FieldValue::List(vec![
            Some("ITEM-ONE".to_owned()),
            Some("ITEM-TWO".to_owned())
        ])
    );
    assert_eq!(
        kinds(&result.diagnostics),
        vec![("items", FieldIssueKind::Truncated)]
    );
}

/// FIELD-13：非 HTML 内容 -> 显式 UNSUPPORTED（说明性结果，不是空成功）。
#[test]
fn fields_13_non_html_body_is_unsupported() {
    let declaration = specs(vec![("title", text("h1"))]);
    let envelope = extract_fields_body(&binary_body(ContentKind::Pdf), &declaration, &options());
    assert_valid(&envelope);
    assert!(envelope.ok);
    assert_eq!(envelope.code, ErrorCode::Unsupported);
    assert_eq!(
        envelope.meta.degraded_from.as_deref(),
        Some("content-type:pdf")
    );
    let result = envelope.data.expect("fields data");
    assert!(result.fields.is_empty());
    assert!(
        result.conclusion.contains("requires HTML"),
        "{}",
        result.conclusion
    );
}

/// FIELD-14：值就是普通 JSON（字符串 / 数组 / null），消费者不需要额外约定。
#[test]
fn fields_14_values_serialize_to_plain_json() {
    assert_eq!(
        serde_json::to_value(FieldValue::Text("x".to_owned())).expect("serialize"),
        serde_json::json!("x")
    );
    assert_eq!(
        serde_json::to_value(FieldValue::List(vec![Some("a".to_owned()), None]))
            .expect("serialize"),
        serde_json::json!(["a", null])
    );
    assert_eq!(
        serde_json::to_value(FieldValue::Null).expect("serialize"),
        serde_json::json!(null)
    );
    let back: FieldValue =
        serde_json::from_value(serde_json::json!(["a", null])).expect("deserialize");
    assert_eq!(back, FieldValue::List(vec![Some("a".to_owned()), None]));
}

/// FIELD-15：既有 `select_matches` 输出不因新参数改变（同一 fixture 逐值对照）。
#[test]
fn fields_15_selector_output_matches_legacy_path() {
    let legacy = select_matches(PAGE, "li.item", &options());
    assert_valid(&legacy);
    assert!(legacy.ok, "{}", legacy.error);
    assert_eq!(legacy.code, ErrorCode::Ok);
    assert!(legacy.meta.degraded_from.is_none());
    let legacy_texts: Vec<String> = legacy
        .data
        .as_ref()
        .expect("legacy data")
        .matches
        .iter()
        .map(|hit| hit.text.clone())
        .collect();
    assert_eq!(legacy_texts, ["ITEM-ONE", "ITEM-TWO", "ITEM-THREE"]);

    let declaration = specs(vec![("items", all("li.item"))]);
    let fields_envelope = extract_fields(PAGE, &declaration, &options());
    assert!(fields_envelope.ok, "{}", fields_envelope.error);
    assert_eq!(fields_envelope.code, ErrorCode::Ok);
    let result = fields_envelope.data.expect("fields data");
    let FieldValue::List(values) = field(&result, "items") else {
        panic!("all=true 必须是数组：{:?}", field(&result, "items"));
    };
    let field_texts: Vec<&str> = values
        .iter()
        .map(|value| value.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(field_texts, legacy_texts);
}

/// FIELD-16：墙钟预算为 0 -> TIMEOUT（字段扫描也受预算约束，不无限跑）。
#[test]
fn fields_16_zero_budget_is_timeout() {
    let mut opts = options();
    opts.max_elapsed_ms = 0;
    let declaration = specs(vec![("title", text("h1"))]);
    let envelope = extract_fields(HEADINGS, &declaration, &opts);
    assert_valid(&envelope);
    assert!(!envelope.ok);
    assert_eq!(envelope.code, ErrorCode::Timeout);
    assert!(envelope.error.contains("budget"), "{}", envelope.error);
}

/// FIELD-17：稳定性声明与信任标记与选择器路径一致。
#[test]
fn fields_17_stability_and_trust() {
    let declaration = specs(vec![("title", text("h1"))]);
    let envelope = extract_fields(HEADINGS, &declaration, &options());
    assert!(envelope.ok, "{}", envelope.error);
    assert_eq!(envelope.meta.trust, Trust::UntrustedPageContent);
    let result = envelope.data.expect("fields data");
    assert_eq!(result.stability, STABILITY_NOTE);
}

/// FIELD-18：全部字段都被选择器守卫拒绝 -> 整体 BLOCKED（与选择器路径同码）。
#[test]
fn fields_18_all_blocked_is_blocked() {
    let oversized = "div ".repeat(200);
    let declaration = specs(vec![("a", text(&oversized)), ("b", text(&oversized))]);
    let envelope = extract_fields(PAGE, &declaration, &options());
    assert_valid(&envelope);
    assert!(!envelope.ok);
    assert_eq!(envelope.code, ErrorCode::Blocked);
    assert!(envelope.error.contains("全部非法"), "{}", envelope.error);
}
