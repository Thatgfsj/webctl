//! 表单模型/编码/解析的单元测试（由 `form.rs` 拆出，仅移位）。

use super::*;

fn first_form(html: &str) -> Form {
    parse_forms(html).into_iter().next().expect("form")
}

#[test]
fn maps_input_types_and_hidden_fields() {
    let html = r#"<form action="/f" method="post">
             <input type="hidden" name="csrf" value="tok">
             <input type="text" name="user" value="默认">
             <input type="checkbox" name="ok" checked>
             <input type="radio" name="r" value="a">
             <input type="radio" name="r" value="b" checked>
             <select name="s"><option value="x">X</option><option value="y" selected>Y</option></select>
             <textarea name="note">hello</textarea>
             <input type="unknown" name="weird" value="v">
           </form>"#;
    let form = first_form(html);
    assert_eq!(form.method, FormMethod::Post);
    assert_eq!(form.field("csrf").unwrap().kind, FieldKind::Hidden);
    assert_eq!(form.field("user").unwrap().value, "默认");
    assert_eq!(form.field("ok").unwrap().value, "on");
    assert!(form.field("ok").unwrap().checked);
    assert_eq!(form.field("s").unwrap().default_select_value(), "y");
    assert_eq!(form.field("note").unwrap().value, "hello");
    assert_eq!(form.field("weird").unwrap().kind, FieldKind::Other);
}

#[test]
fn urlencoded_encoding_preserves_order_and_utf8() {
    let html = r#"<form action="/f"><input type="hidden" name="csrf" value="t1"><input name="q" value="中文 a&b"></form>"#;
    let form = first_form(html);
    let mut fill = form.default_fill();
    fill.set("q", "中文 a&b");
    let body = form.encode(&fill).expect("encode");
    assert_eq!(body.content_type, "application/x-www-form-urlencoded");
    assert_eq!(
        String::from_utf8(body.bytes).expect("utf8"),
        "csrf=t1&q=%E4%B8%AD%E6%96%87+a%26b"
    );
}

#[test]
fn unknown_field_and_missing_required_are_errors() {
    let form = first_form(r#"<form action="/f"><input name="a" required><input name="b"></form>"#);
    let mut fill = form.default_fill();
    fill.set("nope", "1");
    assert_eq!(
        form.encode(&fill),
        Err(FormError::UnknownField("nope".to_owned()))
    );
    let mut fill = form.default_fill();
    fill.set("b", "x");
    assert_eq!(
        form.encode(&fill),
        Err(FormError::MissingRequired("a".to_owned()))
    );
}

#[test]
fn checkbox_and_radio_follow_html_semantics() {
    let html = r#"<form action="/f">
             <input type="checkbox" name="c" value="yes" checked>
             <input type="checkbox" name="d" value="no">
             <input type="radio" name="r" value="a">
             <input type="radio" name="r" value="b" checked>
           </form>"#;
    let form = first_form(html);
    let fill = form.default_fill();
    let body = form.encode(&fill).expect("encode");
    assert_eq!(String::from_utf8(body.bytes).expect("utf8"), "c=yes&r=b");

    let mut fill = form.default_fill();
    fill.check("c", false).select_radio("r", "a");
    let body = form.encode(&fill).expect("encode");
    assert_eq!(String::from_utf8(body.bytes).expect("utf8"), "r=a");
}

#[test]
fn multipart_includes_file_bytes() {
    let html = r#"<form action="/up" method="post" enctype="multipart/form-data"><input type="file" name="doc"></form>"#;
    let form = first_form(html);
    let mut fill = form.default_fill();
    fill.upload(
        "doc",
        FileUpload::new("a.txt", "text/plain", b"hello".to_vec()),
    );
    let body = form.encode(&fill).expect("encode");
    assert!(
        body.content_type
            .starts_with("multipart/form-data; boundary=")
    );
    let text = String::from_utf8_lossy(&body.bytes);
    assert!(text.contains("filename=\"a.txt\""));
    assert!(text.contains("Content-Type: text/plain"));
    assert!(text.contains("hello"));
}

#[test]
fn file_on_urlencoded_form_is_unsupported() {
    let form = first_form(r#"<form action="/up"><input type="file" name="doc"></form>"#);
    let mut fill = form.default_fill();
    fill.upload("doc", FileUpload::new("a.txt", "text/plain", b"x".to_vec()));
    assert_eq!(
        form.encode(&fill),
        Err(FormError::FileNeedsMultipart("doc".to_owned()))
    );
}

#[test]
fn duplicate_checkbox_same_name_submits_only_checked() {
    let first = first_form(
        r#"<form action="/f">
             <input type="checkbox" name="tags" value="a" checked>
             <input type="checkbox" name="tags" value="b">
           </form>"#,
    );
    let body = first.encode(&first.default_fill()).expect("encode");
    assert_eq!(String::from_utf8(body.bytes).expect("utf8"), "tags=a");

    let second = first_form(
        r#"<form action="/f">
             <input type="checkbox" name="tags" value="a">
             <input type="checkbox" name="tags" value="b" checked>
           </form>"#,
    );
    let body = second.encode(&second.default_fill()).expect("encode");
    assert_eq!(String::from_utf8(body.bytes).expect("utf8"), "tags=b");
}

#[test]
fn duplicate_text_inputs_keep_document_order() {
    let form = first_form(
        r#"<form action="/f"><input name="q" value="one"><input name="q" value="two"></form>"#,
    );
    let body = form.encode(&form.default_fill()).expect("encode");
    assert_eq!(String::from_utf8(body.bytes).expect("utf8"), "q=one&q=two");

    // 显式只设一个值时广播到所有同名控件。
    let mut fill = form.default_fill();
    fill.set("q", "x");
    let body = form.encode(&fill).expect("encode");
    assert_eq!(String::from_utf8(body.bytes).expect("utf8"), "q=x&q=x");
}

#[test]
fn select_multiple_submits_all_selected() {
    let form = first_form(
        r#"<form action="/f"><select name="colors" multiple>
             <option value="red">Red</option>
             <option value="green" selected>Green</option>
             <option value="blue" selected>Blue</option>
           </select></form>"#,
    );
    let body = form.encode(&form.default_fill()).expect("encode");
    assert_eq!(
        String::from_utf8(body.bytes).expect("utf8"),
        "colors=green&colors=blue"
    );
}
