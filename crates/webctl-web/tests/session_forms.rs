//! FORM-01…19 的离线集成测试（PLAN v0.4 §2「会话」域，PR 04）。
//!
//! 全部用例只访问 127.0.0.1 上的本地表单站，不访问外网、不启动子进程。
//! fixture 与辅助在 tests/support/session_site.rs / session_helpers.rs（两个二进制共用）。

use webctl_core::ErrorCode;
use webctl_web::session::FormFill;

// ---------------------------------------------------------------------------
// 本地表单站 fixture（手写最小 HTTP/1.1，支持 GET/POST/cookie/重定向/multipart）

#[path = "support/session_helpers.rs"]
mod helpers;
#[path = "support/session_site.rs"]
mod site;

use helpers::engine;
use site::{Site, parse_multipart, parse_pairs_ordered};

// ---------------------------------------------------------------------------
// FORM
// ---------------------------------------------------------------------------

/// FORM-01：单字段填值。
#[tokio::test]
async fn form_01_single_field() {
    let site = Site::start().await;
    let engine = engine("form01");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let mut fill = FormFill::new();
    fill.set("name", "Alice");
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("name").map(String::as_str), Some("Alice"));
}

/// FORM-02：一次填多个字段。
#[tokio::test]
async fn form_02_multiple_fields_at_once() {
    let site = Site::start().await;
    let engine = engine("form02");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let mut fill = FormFill::new();
    fill.set("name", "Bob").set("note", "hello note");
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("name").map(String::as_str), Some("Bob"));
    assert_eq!(recorded.get("note").map(String::as_str), Some("hello note"));
}

/// FORM-03：中文值按 UTF-8 正确编码传输。
#[tokio::test]
async fn form_03_unicode_value() {
    let site = Site::start().await;
    let engine = engine("form03");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let mut fill = FormFill::new();
    fill.set("name", "中文名字");
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("name").map(String::as_str), Some("中文名字"));
}

/// FORM-04：select 选中项与提交值一致。
#[tokio::test]
async fn form_04_select_value() {
    let site = Site::start().await;
    let engine = engine("form04");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(
        recorded.get("color").map(String::as_str),
        Some("green"),
        "默认选中 green"
    );

    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let mut fill = FormFill::new();
    fill.set("color", "blue");
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("color").map(String::as_str), Some("blue"));
}

/// FORM-05：checkbox / radio 勾选与不勾选都符合 HTML 语义。
#[tokio::test]
async fn form_05_checkbox_and_radio_semantics() {
    let site = Site::start().await;
    let engine = engine("form05");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("agree").map(String::as_str), Some("yes"));
    assert_eq!(recorded.get("plan").map(String::as_str), Some("pro"));

    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let mut fill = FormFill::new();
    fill.check("agree", false).select_radio("plan", "free");
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert!(!recorded.contains_key("agree"), "未勾选应不提交");
    assert_eq!(recorded.get("plan").map(String::as_str), Some("free"));
}

/// FORM-06：隐藏字段未被覆盖时保留原值。
#[tokio::test]
async fn form_06_hidden_field_default() {
    let site = Site::start().await;
    let engine = engine("form06");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("lang").map(String::as_str), Some("zh-CN"));
}

/// FORM-07：提交不存在的字段 → 明确错误（不静默忽略、不发起请求）。
#[tokio::test]
async fn form_07_unknown_field_is_error() {
    let site = Site::start().await;
    let engine = engine("form07");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let before = site.hits("/form/landing");
    let mut fill = FormFill::new();
    fill.set("nope", "1");
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Parse);
    assert!(env.error.contains("nope"), "{}", env.error);
    assert_eq!(site.hits("/form/landing"), before, "不得发起请求");
    assert_eq!(env.validate(), Ok(()));
}

/// FORM-08：缺必填字段 → 可读错误（不发起请求）。
#[tokio::test]
async fn form_08_missing_required() {
    let site = Site::start().await;
    let engine = engine("form08");
    let page = engine
        .fetch_page(&site.url("/form/required"))
        .await
        .data
        .expect("page");
    let before = site.hits("/form/required");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Parse);
    assert!(env.error.contains("must"), "{}", env.error);
    assert_eq!(site.hits("/form/required"), before);
}

/// FORM-11：提交后返回最终 URL 与状态可追溯。
#[tokio::test]
async fn form_11_landing_url_and_status() {
    let site = Site::start().await;
    let engine = engine("form11");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let landed = env.data.expect("landed");
    assert_eq!(landed.url, site.url("/form/landing"));
    assert_eq!(landed.status, 200);
    assert!(landed.html.contains("landed"));
}

/// FORM-12：同一 token 重复提交被显式拒绝（客户端幂等保护）。
#[tokio::test]
async fn form_12_duplicate_submission_refused() {
    let site = Site::start().await;
    let engine = engine("form12");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let first = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(first.ok, "{}", first.error);
    let hits_after_first = site.hits("/form/landing");
    let second = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(!second.ok);
    assert_eq!(second.code, ErrorCode::Blocked);
    assert!(second.error.contains("duplicate"), "{}", second.error);
    assert_eq!(
        site.hits("/form/landing"),
        hits_after_first,
        "第二次不得真正发送"
    );
}

/// 守护断言：fixture 的 multipart 解析能识别文件与文本字段（自检）。
#[test]
fn fixture_multipart_parser_sanity() {
    let body = b"--B\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nhi\r\n--B\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nbytes\r\n--B--\r\n";
    let parts = parse_multipart(body, "B");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].name, "title");
    assert_eq!(parts[1].filename.as_deref(), Some("a.txt"));
    assert_eq!(parts[1].content.as_slice(), b"bytes");
}

/// 从保序 pairs 里取出某名字的全部值（同名多值断言）。
fn body_values(pairs: &[(String, String)], name: &str) -> Vec<String> {
    pairs
        .iter()
        .filter(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
        .collect()
}

/// FORM-13：同名多 checkbox 各自独立——第 1 个 checked 只提交 tags=a。
#[tokio::test]
async fn form_13_duplicate_checkbox_first_checked_only() {
    let site = Site::start().await;
    let engine = engine("form13");
    let page = engine
        .fetch_page(&site.url("/form/multi?checked=a"))
        .await
        .data
        .expect("page");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let pairs = parse_pairs_ordered(site.body("/form/multi").expect("body").as_bytes());
    assert_eq!(
        body_values(&pairs, "tags"),
        vec!["a".to_owned()],
        "第 1 个 checked 只应提交 tags=a：{pairs:?}"
    );
}

/// FORM-14：同名多 checkbox 各自独立——第 2 个 checked 只提交 tags=b（不得带 a）。
#[tokio::test]
async fn form_14_duplicate_checkbox_second_checked_only() {
    let site = Site::start().await;
    let engine = engine("form14");
    let page = engine
        .fetch_page(&site.url("/form/multi?checked=b"))
        .await
        .data
        .expect("page");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let pairs = parse_pairs_ordered(site.body("/form/multi").expect("body").as_bytes());
    assert_eq!(
        body_values(&pairs, "tags"),
        vec!["b".to_owned()],
        "第 2 个 checked 只应提交 tags=b：{pairs:?}"
    );
}

/// FORM-15：同名多 text 按出现顺序全部提交（one 在前、two 在后）。
#[tokio::test]
async fn form_15_duplicate_text_inputs_keep_order() {
    let site = Site::start().await;
    let engine = engine("form15");
    let page = engine
        .fetch_page(&site.url("/form/multi?checked=none"))
        .await
        .data
        .expect("page");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let pairs = parse_pairs_ordered(site.body("/form/multi").expect("body").as_bytes());
    assert_eq!(
        body_values(&pairs, "q"),
        vec!["one".to_owned(), "two".to_owned()],
        "同名 text 必须按出现顺序全提交：{pairs:?}"
    );
}

/// FORM-16：select multiple 提交全部 selected 选项（按选项顺序）。
#[tokio::test]
async fn form_16_select_multiple_submits_all_selected() {
    let site = Site::start().await;
    let engine = engine("form16");
    let page = engine
        .fetch_page(&site.url("/form/multi?checked=none"))
        .await
        .data
        .expect("page");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let pairs = parse_pairs_ordered(site.body("/form/multi").expect("body").as_bytes());
    assert_eq!(
        body_values(&pairs, "colors"),
        vec!["green".to_owned(), "blue".to_owned()],
        "select multiple 必须提交全部 selected：{pairs:?}"
    );
}

/// FORM-17：307 重定向保留方法与 POST body。
#[tokio::test]
async fn form_17_307_preserves_post_body() {
    let site = Site::start().await;
    let engine = engine("form17");
    let page = engine
        .fetch_page(&site.url("/form/redirect?status=307"))
        .await
        .data
        .expect("page");
    let mut fill = FormFill::new();
    fill.set("q", "hello");
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(env.ok, "{}", env.error);
    assert_eq!(site.method("/echo/307").as_deref(), Some("POST"));
    let pairs = parse_pairs_ordered(site.body("/echo/307").expect("body").as_bytes());
    assert_eq!(
        body_values(&pairs, "q"),
        vec!["hello".to_owned()],
        "307 后 q=hello 不得丢失：{pairs:?}"
    );
}

/// FORM-18：308 重定向保留方法与 POST body。
#[tokio::test]
async fn form_18_308_preserves_post_body() {
    let site = Site::start().await;
    let engine = engine("form18");
    let page = engine
        .fetch_page(&site.url("/form/redirect?status=308"))
        .await
        .data
        .expect("page");
    let mut fill = FormFill::new();
    fill.set("q", "hello");
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(env.ok, "{}", env.error);
    assert_eq!(site.method("/echo/308").as_deref(), Some("POST"));
    let pairs = parse_pairs_ordered(site.body("/echo/308").expect("body").as_bytes());
    assert_eq!(
        body_values(&pairs, "q"),
        vec!["hello".to_owned()],
        "308 后 q=hello 不得丢失：{pairs:?}"
    );
}

/// FORM-19：303 转 GET 且不带原 POST body。
#[tokio::test]
async fn form_19_303_switches_to_get_without_body() {
    let site = Site::start().await;
    let engine = engine("form19");
    let page = engine
        .fetch_page(&site.url("/form/redirect?status=303"))
        .await
        .data
        .expect("page");
    let mut fill = FormFill::new();
    fill.set("q", "hello");
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(env.ok, "{}", env.error);
    assert_eq!(site.method("/echo/303").as_deref(), Some("GET"));
    let body = site.body("/echo/303").expect("body");
    assert!(body.is_empty(), "303 后不得携带 body，实际 {body:?}");
}
