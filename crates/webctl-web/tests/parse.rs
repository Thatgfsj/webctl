//! PARSE-01…PARSE-16 的离线集成测试（docs/TESTING.md §2.4）。
//!
//! 全部用例只读本地 fixture 文件，不联网、不启动子进程、不依赖浏览器。
//! 用例编号与 TESTING.md 对齐；PARSE-12 归属 PR 06，这里只覆盖选择器保序的
//! 内核语义。

use std::time::Instant;

use webctl_core::{Envelope, ErrorCode, Trust};
use webctl_web::parse::html::{render_markdown, render_plain_text};
use webctl_web::parse::structured::{MAX_XML_DEPTH, max_xml_depth};
use webctl_web::parse::{
    Parsed, extract_article, extract_article_fallback, extract_forms, extract_links,
    extract_tables, parse_body, parse_json, parse_xml, select,
};
use webctl_web::{Body, CharsetSource, ContentKind, decode_body};

const ARTICLE: &str = include_str!("fixtures/parse/article.html");
const MALFORMED: &str = include_str!("fixtures/parse/malformed.html");
const LINKS: &str = include_str!("fixtures/parse/links.html");
const LINKS_NOBASE: &str = include_str!("fixtures/parse/links_nobase.html");
const TABLES: &str = include_str!("fixtures/parse/tables.html");
const SPANS: &str = include_str!("fixtures/parse/spans.html");
const FORMS: &str = include_str!("fixtures/parse/forms.html");
const MARKDOWN: &str = include_str!("fixtures/parse/markdown.html");
const DATA_JSON: &str = include_str!("fixtures/parse/data.json");
const DATA_XML: &str = include_str!("fixtures/parse/data.xml");

const DOC_URL: &str = "https://doc.example/dir/doc.html";

fn assert_valid<T: std::fmt::Debug>(envelope: &Envelope<T>) {
    assert_eq!(
        envelope.validate(),
        Ok(()),
        "envelope invariants: {envelope:?}"
    );
}

fn text_body(kind: ContentKind, text: &str) -> Body {
    Body {
        kind,
        content_type: Some(format!("{}; charset=utf-8", kind.as_str())),
        byte_len: text.len(),
        declared_len: None,
        truncated: false,
        text: Some(text.to_owned()),
        charset: Some("UTF-8".to_owned()),
        charset_source: Some(CharsetSource::Utf8Valid),
        decode_had_errors: false,
    }
}

fn binary_body(kind: ContentKind) -> Body {
    Body {
        kind,
        content_type: Some("application/octet-stream".to_owned()),
        byte_len: 8,
        declared_len: None,
        truncated: false,
        text: None,
        charset: None,
        charset_source: None,
        decode_had_errors: false,
    }
}

/// PARSE-01：正文提取包含首尾句，且信噪比干净。
#[test]
fn parse_01_article_extraction() {
    let env = extract_article(ARTICLE, Some(DOC_URL));
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.meta.trust, Trust::UntrustedPageContent);
    assert_eq!(env.code, ErrorCode::Ok);
    let article = env.data.as_ref().expect("article data");
    assert_eq!(article.extractor, "dom_smoothie");
    assert!(
        article.text.contains("FIRSTBODYSENTENCE"),
        "{}",
        article.text
    );
    assert!(
        article.text.contains("LASTBODYSENTENCE"),
        "{}",
        article.text
    );
    assert!(!article.text.contains("NAVIGATIONMARKER"));
    assert!(!article.text.contains("FOOTERMARKER"));
    assert!(!article.text.contains("SIDEBARMARKER"));
}

/// PARSE-02：导航/页脚/侧栏文本不进入 markdown 正文。
#[test]
fn parse_02_noise_filtered_from_markdown() {
    let env = extract_article(ARTICLE, Some(DOC_URL));
    assert!(env.ok, "{}", env.error);
    let article = env.data.as_ref().expect("article data");
    assert!(!article.markdown.contains("NAVIGATIONMARKER"));
    assert!(!article.markdown.contains("FOOTERMARKER"));
    assert!(!article.markdown.contains("SIDEBARMARKER"));
    assert!(article.markdown.contains("FIRSTBODYSENTENCE"));
    assert!(article.markdown.contains("LASTBODYSENTENCE"));
}

/// PARSE-03：畸形 HTML 不 panic，仍能提取到两端标记。
#[test]
fn parse_03_malformed_html() {
    let env = extract_article(MALFORMED, None);
    assert_valid(&env);
    assert!(env.ok, "malformed HTML should still extract: {}", env.error);
    let article = env.data.as_ref().expect("article data");
    assert!(article.text.contains("MALFORMED_FIRST"), "{}", article.text);
    assert!(article.text.contains("MALFORMED_LAST"), "{}", article.text);
}

/// PARSE-04：GBK 页面经 PR 02 解码后正文无乱码。
#[test]
fn parse_04_gbk_decodes_without_mojibake() {
    const GBK_BYTES: &[u8] = &[
        0xd6, 0xd0, 0xce, 0xc4, 0xb1, 0xe0, 0xc2, 0xeb, 0xb2, 0xe2, 0xca, 0xd4, 0xa3, 0xba, 0xc4,
        0xe3, 0xba, 0xc3, 0xa3, 0xac, 0xca, 0xc0, 0xbd, 0xe7,
    ];
    let sentence = "中文编码测试：你好，世界";
    let mut html = b"<html><body><article><h1>".to_vec();
    html.extend_from_slice(GBK_BYTES);
    html.extend_from_slice(b"</h1><p>");
    for _ in 0..3 {
        html.extend_from_slice(GBK_BYTES);
        // GBK 的全角句号（0xA1 0xA3），保持 fixture 全字节可控。
        html.extend_from_slice(&[0xa1, 0xa3]);
    }
    html.extend_from_slice(b"</p></article></body></html>");
    let decoded = decode_body(&html, Some("text/html; charset=gbk"));
    assert_eq!(decoded.encoding, "GBK");
    let env = extract_article(&decoded.text, None);
    assert!(env.ok, "{}", env.error);
    let article = env.data.as_ref().expect("article data");
    assert!(article.text.contains(sentence), "{}", article.text);
    assert!(!article.text.contains('�'));
    assert!(article.markdown.contains(sentence));
}

/// PARSE-04（日文）：Shift_JIS 页面经 PR 02 解码后正文无乱码。
#[test]
fn parse_04b_shift_jis_decodes_without_mojibake() {
    const SJIS_BYTES: &[u8] = &[
        0x93, 0xfa, 0x96, 0x7b, 0x8c, 0xea, 0x82, 0xcc, 0x83, 0x65, 0x83, 0x58, 0x83, 0x67, 0x81,
        0x46, 0x82, 0xb1, 0x82, 0xf1, 0x82, 0xc9, 0x82, 0xbf, 0x82, 0xcd,
    ];
    let sentence = "日本語のテスト：こんにちは";
    let mut html = b"<html><body><article><h1>".to_vec();
    for _ in 0..2 {
        html.extend_from_slice(SJIS_BYTES);
        html.extend_from_slice(b"<br>");
    }
    html.extend_from_slice(b"</h1></article></body></html>");
    let decoded = decode_body(&html, Some("text/html; charset=shift_jis"));
    assert_eq!(decoded.encoding, "Shift_JIS");
    let env = extract_article(&decoded.text, None);
    assert!(env.ok, "{}", env.error);
    let article = env.data.as_ref().expect("article data");
    assert!(article.text.contains(sentence), "{}", article.text);
    assert!(!article.text.contains('�'));
}

/// PARSE-05：链接数量、绝对化、rel、文本。
#[test]
fn parse_05_links_absolute_and_rel() {
    let env = extract_links(LINKS_NOBASE, Some(DOC_URL));
    assert_valid(&env);
    assert!(env.ok);
    let links = env.data.as_ref().expect("links");
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].raw_href, "page.html");
    assert_eq!(links[0].href, "https://doc.example/dir/page.html");
    assert!(links[0].absolute);
    assert_eq!(links[0].text, "Relative");
    assert_eq!(links[0].rel, vec!["next", "nofollow"]);
    assert_eq!(links[0].title.as_deref(), Some("relative"));
    assert_eq!(links[1].href, "https://doc.example/root.html");
}

/// PARSE-06：base href 参与相对链接解析且优先于文档 URL。
#[test]
fn parse_06_base_href_wins() {
    let env = extract_links(LINKS, Some(DOC_URL));
    assert!(env.ok);
    let links = env.data.as_ref().expect("links");
    assert_eq!(links.len(), 6);
    assert_eq!(links[0].href, "https://fixture.example/base/page.html");
    assert_eq!(links[1].href, "https://fixture.example/root.html");
    assert_eq!(links[2].href, "https://other.example/x");
    assert_eq!(links[2].target.as_deref(), Some("_blank"));
    assert_eq!(links[3].href, "https://fixture.example/base/#frag");
    assert_eq!(links[4].href, "https://cdn.example/a.js");
    assert_eq!(links[5].href, "https://fixture.example/base/map.html");
}

/// PARSE-07：行列数与逐格文本。
#[test]
fn parse_07_table_grid() {
    let env = extract_tables(TABLES);
    assert_valid(&env);
    assert!(env.ok);
    let tables = env.data.as_ref().expect("tables");
    assert_eq!(tables.len(), 1);
    let table = &tables[0];
    assert_eq!(table.caption.as_deref(), Some("Simple"));
    assert_eq!(table.rows, 3);
    assert_eq!(table.columns, 3);
    let text = |row: usize, col: usize| table.cells[table.grid[row][col]].text.clone();
    assert_eq!(text(0, 0), "Name");
    assert!(table.cells[table.grid[0][0]].header);
    assert_eq!(text(1, 0), "Alice");
    assert_eq!(text(2, 2), "Tokyo");
}

/// PARSE-08：rowspan/colspan 不丢格，覆盖位置回指起点格。
#[test]
fn parse_08_spans_have_deterministic_coverage() {
    let env = extract_tables(SPANS);
    assert!(env.ok, "{}", env.error);
    let table = &env.data.as_ref().expect("tables")[0];
    assert_eq!(table.rows, 4);
    assert_eq!(table.columns, 4);
    // colspan：Sales 覆盖两列。
    assert_eq!(table.grid[0][1], table.grid[0][2]);
    // rowspan：Region / Total 覆盖两行。
    assert_eq!(table.grid[0][0], table.grid[1][0]);
    assert_eq!(table.grid[0][3], table.grid[1][3]);
    assert_eq!(table.cells[table.grid[0][0]].row_span, 2);
    assert_eq!(table.cells[table.grid[0][1]].col_span, 2);
    // 每个网格位置都有确定归属，且没有合成空格子。
    for (row, cells) in table.grid.iter().enumerate() {
        for (col, cell) in cells.iter().enumerate() {
            let cell = &table.cells[*cell];
            assert!(!cell.text.is_empty(), "grid[{row}][{col}] lost its cell");
            assert!(!cell.synthetic);
        }
    }
}

/// PARSE-09：字段名/类型/默认值/action/method 全对，可直接喂给 fill_form。
#[test]
fn parse_09_form_fields() {
    let env = extract_forms(FORMS, Some(DOC_URL));
    assert_valid(&env);
    assert!(env.ok);
    let forms = env.data.as_ref().expect("forms");
    assert_eq!(forms.len(), 2);
    let form = &forms[0];
    assert_eq!(form.id.as_deref(), Some("login"));
    assert_eq!(form.action.as_deref(), Some("/login"));
    assert_eq!(
        form.action_url.as_deref(),
        Some("https://doc.example/login")
    );
    assert_eq!(form.method, "post");
    assert_eq!(form.fields.len(), 7);

    let field = |name: &str| {
        form.fields
            .iter()
            .find(|field| field.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("missing field {name}"))
    };
    let user = field("user");
    assert_eq!(user.field_type, "text");
    assert_eq!(user.default_value.as_deref(), Some("alice"));
    assert!(user.required);
    assert_eq!(user.label.as_deref(), Some("User name"));
    assert_eq!(field("pass").field_type, "password");
    assert_eq!(field("csrf").field_type, "hidden");
    assert!(field("remember").checked);
    assert_eq!(field("remember").label.as_deref(), Some("Remember me"));
    let country = field("country");
    assert_eq!(country.field_type, "select");
    assert!(country.multiple);
    assert_eq!(country.default_value.as_deref(), Some("fr"));
    assert_eq!(country.options.len(), 2);
    assert!(country.options[0].selected);
    assert!(country.options[1].disabled);
    assert_eq!(field("bio").field_type, "textarea");
    assert_eq!(field("bio").default_value.as_deref(), Some("hello"));
    let go = field("go");
    assert_eq!(go.field_type, "submit");
    assert_eq!(go.default_value.as_deref(), Some("1"));

    assert_eq!(forms[1].method, "get");
    assert_eq!(
        forms[1].action_url.as_deref(),
        Some("https://other.example/post")
    );
}

/// PARSE-09（补充）：缺省 method/action/enctype 与无 name 字段不丢。
#[test]
fn parse_09b_form_defaults_and_unnamed_fields() {
    let env = extract_forms(
        "<form><input name=\"q\"><input type=\"submit\" value=\"Send\"></form>",
        None,
    );
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    let forms = env.data.as_ref().expect("forms");
    assert_eq!(forms.len(), 1);
    let form = &forms[0];
    assert_eq!(form.method, "get");
    assert_eq!(form.action, None);
    assert_eq!(form.action_url, None);
    assert_eq!(form.enctype, "application/x-www-form-urlencoded");
    assert_eq!(form.fields.len(), 2);
    assert_eq!(form.fields[0].name.as_deref(), Some("q"));
    assert_eq!(form.fields[0].field_type, "text");
    assert_eq!(form.fields[1].name, None);
    assert_eq!(form.fields[1].field_type, "submit");
    assert_eq!(form.fields[1].default_value.as_deref(), Some("Send"));
}

/// PARSE-10：markdown 视图输出标题/列表/链接/强调标记。
#[test]
fn parse_10_markdown_rendering() {
    let markdown = render_markdown(MARKDOWN);
    assert!(markdown.contains("# Title"), "{markdown}");
    assert!(markdown.contains("- one"), "{markdown}");
    assert!(markdown.contains("- two"), "{markdown}");
    assert!(markdown.contains("**bold**"), "{markdown}");
    assert!(markdown.contains("*italic*"), "{markdown}");
    assert!(
        markdown.contains("[link text](https://example.com/a)"),
        "{markdown}"
    );
    let plain = render_plain_text(MARKDOWN);
    assert!(plain.contains("Title"), "{plain}");
    assert!(plain.contains("link text"), "{plain}");
}

/// PARSE-11：JSON / XML 结构化解析并保留原始文本。
#[test]
fn parse_11_structured_data() {
    let json_env = parse_json(DATA_JSON);
    assert_valid(&json_env);
    assert!(json_env.ok, "{}", json_env.error);
    let json = json_env.data.as_ref().expect("json");
    assert_eq!(json.format, "json");
    assert_eq!(json.raw, DATA_JSON);
    assert_eq!(json.value["name"], "webctl");
    assert_eq!(json.value["nested"]["n"], 3);

    let xml_env = parse_xml(DATA_XML);
    assert_valid(&xml_env);
    assert!(xml_env.ok, "{}", xml_env.error);
    let xml = xml_env.data.as_ref().expect("xml");
    assert_eq!(xml.format, "xml");
    assert_eq!(xml.root.as_deref(), Some("catalog"));
    assert_eq!(xml.raw, DATA_XML);
    assert_eq!(xml.value["tag"], "catalog");
    let books = xml.value["children"].as_array().expect("children");
    assert_eq!(books.len(), 2);
    assert_eq!(books[0]["tag"], "book");
    assert_eq!(books[0]["attrs"]["id"], "1");
    assert_eq!(books[0]["children"][0], "Rust");
}

/// PARSE-12（PR 06 的内核部分）：多命中保序。
#[test]
fn parse_12_select_is_ordered() {
    let env = select(MARKDOWN, "li");
    assert_valid(&env);
    assert!(env.ok);
    let hits = env.data.as_ref().expect("hits");
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].index, 0);
    assert_eq!(hits[0].tag, "li");
    assert_eq!(hits[0].text, "one");
    assert_eq!(hits[1].text, "two");
}

/// PARSE-13 / ENV-10：零命中是 ok=true 空结果；解析失败是 code=PARSE。
#[test]
fn parse_13_zero_hits_vs_parse_failure() {
    let zero = select(ARTICLE, "p.never-matches");
    assert_valid(&zero);
    assert!(zero.ok);
    assert!(zero.data.as_ref().expect("hits").is_empty());
    assert_eq!(zero.code, ErrorCode::Ok);

    let invalid = select(ARTICLE, "div[");
    assert_valid(&invalid);
    assert!(!invalid.ok);
    assert_eq!(invalid.code, ErrorCode::Parse);
    assert!(invalid.data.is_none());
    assert!(!invalid.error.is_empty());

    let empty_article = extract_article("", None);
    assert_valid(&empty_article);
    assert!(!empty_article.ok);
    assert_eq!(empty_article.code, ErrorCode::Parse);

    let broken_json = parse_json("{bad");
    assert_valid(&broken_json);
    assert!(!broken_json.ok);
    assert_eq!(broken_json.code, ErrorCode::Parse);

    let zero_links = extract_links("<html><body><p>no links</p></body></html>", None);
    assert_valid(&zero_links);
    assert!(zero_links.ok);
    assert!(zero_links.data.as_ref().expect("links").is_empty());
    assert_eq!(zero_links.code, ErrorCode::Ok);
}

/// PARSE-14：100KB 级 HTML 解析 < 500ms，且耗时进入 meta.elapsed_ms。
#[test]
fn parse_14_budget_on_100kb() {
    let mut html = String::from("<html><body><article><h1>Large fixture</h1>");
    while html.len() < 100 * 1024 {
        html.push_str(
            "<p>Filler paragraph with ordinary words so the readability algorithm has real content to score.</p>",
        );
    }
    html.push_str("<p>LARGE_LAST</p></article></body></html>");
    assert!(html.len() >= 100 * 1024);

    let started = Instant::now();
    let env = extract_article(&html, None);
    let measured = started.elapsed().as_millis() as u64;
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    assert!(measured < 500, "100KB parse took {measured}ms");
    assert!(
        env.meta.elapsed_ms <= measured + 50,
        "meta.elapsed_ms={} measured={measured}",
        env.meta.elapsed_ms
    );
}

/// PARSE-15：非 HTML 输入显式 UNSUPPORTED 或路由到正确处理器。
#[test]
fn parse_15_content_type_routing() {
    let binary = parse_body(&binary_body(ContentKind::Binary), None);
    assert_valid(&binary);
    assert_eq!(binary.code, ErrorCode::Unsupported);
    assert!(binary.code.is_degradation());
    assert_eq!(
        binary.meta.degraded_from.as_deref(),
        Some("content-type:binary")
    );
    match binary.data.as_ref().expect("data") {
        Parsed::Unsupported { kind, .. } => assert_eq!(kind, "binary"),
        other => panic!("unexpected {other:?}"),
    }

    let pdf = parse_body(&binary_body(ContentKind::Pdf), None);
    assert_eq!(pdf.code, ErrorCode::Unsupported);

    let json = parse_body(&text_body(ContentKind::Json, DATA_JSON), None);
    assert_valid(&json);
    assert!(json.ok);
    assert_eq!(json.code, ErrorCode::Ok);
    match json.data.as_ref().expect("data") {
        Parsed::Json(structured) => assert_eq!(structured.format, "json"),
        other => panic!("unexpected {other:?}"),
    }

    let html = parse_body(&text_body(ContentKind::Html, ARTICLE), Some(DOC_URL));
    assert!(html.ok, "{}", html.error);
    match html.data.as_ref().expect("data") {
        Parsed::Html(article) => assert!(article.text.contains("FIRSTBODYSENTENCE")),
        other => panic!("unexpected {other:?}"),
    }

    let xml = parse_body(&text_body(ContentKind::Xml, DATA_XML), None);
    assert_valid(&xml);
    assert!(xml.ok, "{}", xml.error);
    assert_eq!(xml.code, ErrorCode::Ok);
    match xml.data.as_ref().expect("data") {
        Parsed::Xml(structured) => assert_eq!(structured.root.as_deref(), Some("catalog")),
        other => panic!("unexpected {other:?}"),
    }
}

/// PARSE-16：>1000 层嵌套不栈溢出。
#[test]
fn parse_16_deep_nesting() {
    let depth = 2000;
    let deep = format!(
        "<html><body>{}<p>DEEP_TEXT</p>{}</body></html>",
        "<div>".repeat(depth),
        "</div>".repeat(depth)
    );
    let env = extract_article(&deep, None);
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.code, ErrorCode::Degraded);
    assert_eq!(env.meta.degraded_from.as_deref(), Some("dom_smoothie"));
    let article = env.data.as_ref().expect("article");
    assert!(article.text.contains("DEEP_TEXT"), "{}", article.text);
}

/// PARSE-16（XML 路径）：≥1000 层嵌套不栈溢出——线性深度预检在进入
/// roxmltree 递归 tokenizer 之前就拒绝，返回明确错误码而非进程 abort。
#[test]
fn parse_16_deep_xml_nesting_is_rejected() {
    let depth = 1000;
    let deep = format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
    assert_eq!(deep.len(), depth * 7);

    let env = parse_xml(&deep);
    assert_valid(&env);
    assert!(!env.ok, "超深 XML 必须显式失败");
    assert_eq!(env.code, ErrorCode::Parse);
    assert!(env.data.is_none());
    assert!(
        env.error.contains("nesting depth"),
        "错误应说明深度拒绝：{}",
        env.error
    );

    // 路由层同样拿到 PARSE，证明预检在解析入口处生效。
    let routed = parse_body(&text_body(ContentKind::Xml, &deep), None);
    assert_valid(&routed);
    assert!(!routed.ok);
    assert_eq!(routed.code, ErrorCode::Parse);
}

/// PARSE-16（XML 边界）：100 层正常解析；阈值内可解析；阈值外与 130 层拒绝。
#[test]
fn parse_16_xml_depth_boundary() {
    let parse_at = |depth: usize| {
        let xml = format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
        parse_xml(&xml)
    };

    let shallow = parse_at(100);
    assert_valid(&shallow);
    assert!(shallow.ok, "{}", shallow.error);
    assert_eq!(
        shallow.data.as_ref().expect("xml").root.as_deref(),
        Some("a")
    );

    let at_limit = parse_at(MAX_XML_DEPTH);
    assert_valid(&at_limit);
    assert!(
        at_limit.ok,
        "阈值 {} 层应当可解析：{}",
        MAX_XML_DEPTH, at_limit.error
    );

    let over_limit = parse_at(MAX_XML_DEPTH + 1);
    assert_valid(&over_limit);
    assert!(!over_limit.ok);
    assert_eq!(over_limit.code, ErrorCode::Parse);
    assert!(
        over_limit.error.contains("nesting depth"),
        "{}",
        over_limit.error
    );

    let rejected = parse_at(130);
    assert_valid(&rejected);
    assert!(!rejected.ok);
    assert_eq!(rejected.code, ErrorCode::Parse);
}

/// 深度预检是线性的字节扫描，必须忽略注释 / CDATA / 处理指令 / 属性引号里
/// 的 `<`——草率正则匹配会在这里误判。
#[test]
fn parse_16_xml_depth_precheck_ignores_markup_noise() {
    let noise = "<n>".repeat(1000);

    // 属性值里的 `<`（XML 本身非法）与注释 / CDATA / PI 内的假标签都不计数。
    let scan_only =
        format!("<root a=\"{noise}\"><!--{noise}--><![CDATA[{noise}]]><?pi {noise}?><child/>");
    assert_eq!(max_xml_depth(&scan_only), 1, "噪声被误判为嵌套");

    // 合法文档：噪声同样不触发深度拒绝，且能正常解析。
    let document =
        format!("<root><!--{noise}--><![CDATA[{noise}]]><?pi {noise}?><child>ok</child></root>");
    assert_eq!(max_xml_depth(&document), 2);
    let env = parse_xml(&document);
    assert_valid(&env);
    assert!(env.ok, "噪声文档被误拒绝：{}", env.error);
    let value = &env.data.as_ref().expect("xml").value;
    assert_eq!(value["tag"], "root");
    let children = value["children"].as_array().expect("children");
    assert!(
        children.iter().any(|child| child["tag"] == "child"),
        "{value}"
    );
}

/// 回退路径是显式降级：ok=true 但 code=DEGRADED 且 degraded_from 非空。
#[test]
fn fallback_is_explicit_degradation() {
    let env = extract_article_fallback(MARKDOWN, None);
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.code, ErrorCode::Degraded);
    assert_eq!(env.meta.degraded_from.as_deref(), Some("dom_smoothie"));
    let article = env.data.as_ref().expect("article");
    assert_eq!(article.extractor, "fallback");
    assert!(article.markdown.contains("# Title"));
}

fn golden(name: &str, actual: &serde_json::Value) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/parse/golden");
    let path = dir.join(format!("{name}.json"));
    if std::env::var_os("WEBCTL_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(&dir).expect("golden dir");
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(actual).expect("json")),
        )
        .expect("write golden");
    }
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("missing golden {}: {error}", path.display()));
    let expected: serde_json::Value = serde_json::from_str(&raw).expect("parse golden");
    assert_eq!(actual, &expected, "golden mismatch: {}", path.display());
}

/// PARSE-05/06/07/08/09 的 golden：结构化输出被冻结，改动必须显式更新。
#[test]
fn golden_outputs() {
    let links = extract_links(LINKS, Some(DOC_URL));
    assert!(links.ok, "{}", links.error);
    golden(
        "links",
        &serde_json::to_value(links.data.expect("links")).expect("json"),
    );

    let tables = extract_tables(TABLES);
    assert!(tables.ok, "{}", tables.error);
    golden(
        "tables",
        &serde_json::to_value(tables.data.expect("tables")).expect("json"),
    );

    let forms = extract_forms(FORMS, Some(DOC_URL));
    assert!(forms.ok, "{}", forms.error);
    golden(
        "forms",
        &serde_json::to_value(forms.data.expect("forms")).expect("json"),
    );
}
