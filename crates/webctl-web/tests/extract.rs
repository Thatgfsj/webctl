//! PR 06 抽取引擎的离线集成测试（docs/TESTING.md §2.7 EXTRACT-01…17、
//! §2.4 PARSE-12、§2.8 FORM-09）。
//!
//! 全部用例只读本地 fixture / 内存字符串，不联网、不启动子进程。
//! 用例编号与 TESTING.md 对齐；本文件是 PR 06 的验收证据。

use std::time::Instant;

use webctl_core::{Envelope, ErrorCode, Trust};
use webctl_web::extract::{
    ExtractOptions, LinkFilter, STABILITY_NOTE, extract_body, form_by_index, forms, links, search,
    select_matches, tables,
};
use webctl_web::extract::{SearchOptions, SearchResult};
use webctl_web::parse::{extract_forms, extract_links, extract_tables};
use webctl_web::session::form::{FormFill, parse_forms};
use webctl_web::{Body, ContentKind};

const PAGE: &str = include_str!("fixtures/extract/page.html");
const MULTI_FORM: &str = include_str!("fixtures/extract/multi_form.html");
const TABLES: &str = include_str!("fixtures/parse/tables.html");
const LINKS: &str = include_str!("fixtures/parse/links.html");
const FORMS: &str = include_str!("fixtures/parse/forms.html");

const DOC_URL: &str = "https://doc.example/dir/doc.html";
const LINKS_BASE: &str = "https://fixture.example/base/";

fn options() -> ExtractOptions {
    ExtractOptions::default()
}

fn assert_valid<T: std::fmt::Debug>(envelope: &Envelope<T>) {
    assert_eq!(
        envelope.validate(),
        Ok(()),
        "envelope invariants: {envelope:?}"
    );
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

/// EXTRACT-01：每条命中带 tag / 同级索引 / 祖先链（PARSE-12 的结构化路径部分）。
#[test]
fn extract_01_structured_path() {
    let env = select_matches(PAGE, "li.item", &options());
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.meta.trust, Trust::UntrustedPageContent);
    assert_eq!(env.code, ErrorCode::Ok);
    let result = env.data.expect("extract data");
    assert_eq!(result.matches.len(), 3);

    let first = &result.matches[0];
    assert_eq!(first.tag, "li");
    assert_eq!(first.path.tag, "li");
    assert_eq!(first.path.sibling_index, 0);
    assert_eq!(first.path.depth, 4);
    let ancestors: Vec<&str> = first
        .path
        .ancestors
        .iter()
        .map(|segment| segment.tag.as_str())
        .collect();
    assert_eq!(ancestors, ["html", "body", "main", "ul"]);
    assert!(
        first
            .path
            .css
            .ends_with("ul:nth-of-type(4) > li:nth-of-type(1)"),
        "{}",
        first.path.css
    );
    assert_eq!(result.matches[1].path.sibling_index, 1);
    assert_eq!(result.matches[2].path.sibling_index, 2);
}

/// EXTRACT-02：命中附文本上下文（前后若干字符）。
#[test]
fn extract_02_context() {
    let mut opts = options();
    opts.context_chars = 40;
    let env = select_matches(PAGE, "span.target", &opts);
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    let result = env.data.expect("extract data");
    assert_eq!(result.matches.len(), 1);
    let hit = &result.matches[0];
    assert!(hit.text.contains("GAMMAMARK"), "{}", hit.text);
    assert!(hit.before.contains("BETAMARK"), "before={}", hit.before);
    assert!(hit.after.contains("ITEM-ONE"), "after={}", hit.after);
}

/// EXTRACT-03：多命中与文档序一致。
#[test]
fn extract_03_document_order() {
    let env = select_matches(PAGE, "li.item", &options());
    assert!(env.ok, "{}", env.error);
    let result = env.data.expect("extract data");
    let texts: Vec<&str> = result.matches.iter().map(|hit| hit.text.as_str()).collect();
    assert_eq!(texts, ["ITEM-ONE", "ITEM-TWO", "ITEM-THREE"]);
    let indexes: Vec<usize> = result.matches.iter().map(|hit| hit.index).collect();
    assert_eq!(indexes, [0, 1, 2]);
}

/// EXTRACT-04 / PARSE-12：字符预算超限时截断且标记 code=LIMIT。
#[test]
fn extract_04_char_budget_truncates_and_marks() {
    let mut opts = options();
    opts.context_chars = 0;
    opts.max_results = 10;
    opts.char_budget = 8;
    let env = select_matches(PAGE, "li.item", &opts);
    assert_valid(&env);
    assert!(env.ok);
    assert_eq!(env.code, ErrorCode::Limit);
    assert_eq!(env.meta.degraded_from.as_deref(), Some("char-budget"));
    let result = env.data.expect("extract data");
    assert_eq!(result.matches.len(), 1);
    assert!(result.truncated);
    assert!(result.truncated_reasons.contains(&"char-budget".to_owned()));
    assert_eq!(result.matches[0].text, "ITEM-ONE");
}

/// EXTRACT-05：命中数上限截断并标记。
#[test]
fn extract_05_max_results_truncates_and_marks() {
    let mut opts = options();
    opts.max_results = 2;
    let env = select_matches(PAGE, "li.item", &opts);
    assert_valid(&env);
    assert!(env.ok);
    assert_eq!(env.code, ErrorCode::Limit);
    assert_eq!(env.meta.degraded_from.as_deref(), Some("max-results"));
    let result = env.data.expect("extract data");
    assert_eq!(result.matches.len(), 2);
    assert!(result.more_matches);
    assert!(result.truncated_reasons.contains(&"max-results".to_owned()));
}

/// EXTRACT-06：零命中是合法空结果，不是失败。
#[test]
fn extract_06_zero_hits_is_ok_empty() {
    let env = select_matches(PAGE, "article.nope", &options());
    assert_valid(&env);
    assert!(env.ok);
    assert_eq!(env.code, ErrorCode::Ok);
    assert!(env.meta.degraded_from.is_none());
    let result = env.data.expect("extract data");
    assert!(result.matches.is_empty());
    assert!(result.observed_matches == 0);
    assert!(
        result.conclusion.contains("0 elements"),
        "{}",
        result.conclusion
    );
    assert!(result.conclusion.contains("valid empty result"));
}

/// EXTRACT-07：非法选择器 → PARSE，不 panic、不返回空数组冒充成功。
#[test]
fn extract_07_invalid_selector_is_parse() {
    for bad in ["", "div[", ":not(", "a >> b"] {
        let env = select_matches(PAGE, bad, &options());
        assert_valid(&env);
        assert!(!env.ok, "selector {bad:?} must fail");
        assert_eq!(env.code, ErrorCode::Parse, "selector {bad:?}");
        assert!(env.data.is_none());
        assert!(!env.error.is_empty());
    }
}

/// EXTRACT-08：search 文本命中数 / 位置 / 上下文正确。
#[test]
fn extract_08_search_text_positions_and_context() {
    let text = "alpha beta alpha gamma\nsecond line alpha";
    let opts = SearchOptions {
        context_chars: 5,
        ..SearchOptions::default()
    };
    let env = search(text, "alpha", &opts);
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    let result: SearchResult = env.data.expect("search data");
    assert_eq!(result.matches.len(), 3);
    assert_eq!(result.observed_matches, 3);

    let first = &result.matches[0];
    assert_eq!((first.start, first.end), (0, 5));
    assert_eq!((first.line, first.column), (1, 1));
    assert_eq!(first.matched, "alpha");

    let second = &result.matches[1];
    assert_eq!((second.start, second.end), (11, 16));
    assert_eq!((second.line, second.column), (1, 12));

    let third = &result.matches[2];
    assert_eq!(third.line, 2);
    assert_eq!(third.column, 13);
    assert_eq!(third.before, "line");
    assert_eq!(third.after, "");
}

/// EXTRACT-09：正则模式可用；非法正则 → 明确 PARSE 错误。
#[test]
fn extract_09_search_regex_and_invalid_pattern() {
    let opts = SearchOptions {
        regex: true,
        ..SearchOptions::default()
    };
    let env = search("a1 b22 c333", r"\d+", &opts);
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.data.expect("search data").matches.len(), 3);

    let env = search("abc", "(", &opts);
    assert_valid(&env);
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Parse);
    assert!(
        env.error.contains("invalid regular expression"),
        "{}",
        env.error
    );

    let long = SearchOptions {
        regex: true,
        ..SearchOptions::default()
    };
    let env = search("abc", &"(".repeat(1000), &long);
    assert_valid(&env);
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Blocked);
}

/// EXTRACT-10：tables 结构与 fixture / PARSE golden 全等。
#[test]
fn extract_10_tables_match_fixture() {
    let env = tables(TABLES, &options());
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    let actual_tables = env.data.expect("tables data");
    let parse_tables = extract_tables(TABLES).data.expect("parse tables");
    assert_eq!(actual_tables, parse_tables);

    let golden_raw = include_str!("fixtures/parse/golden/tables.json");
    let expected: serde_json::Value = serde_json::from_str(golden_raw).expect("golden json");
    let actual = serde_json::to_value(&actual_tables).expect("serialize tables");
    assert_eq!(actual, expected);
}

/// EXTRACT-11：forms 输出可直接喂给 fill_form（session 的 FormFill / Form::encode）。
#[test]
fn extract_11_forms_feed_fill_form() {
    let env = forms(FORMS, Some(DOC_URL), &options());
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    let extracted = env.data.expect("forms data");
    assert_eq!(extracted.len(), 2);
    let target = &extracted[0];
    assert_eq!(target.action.as_deref(), Some("/login"));
    assert_eq!(target.method, "post");

    let mut fill = FormFill::new();
    for field in &target.fields {
        let Some(name) = field.name.as_deref() else {
            continue;
        };
        if field.disabled {
            continue;
        }
        match field.field_type.as_str() {
            "checkbox" | "radio" => {
                fill.check(name, field.checked);
            }
            _ => {
                if let Some(value) = &field.default_value {
                    fill.set(name, value.clone());
                }
            }
        }
    }

    let session_forms = parse_forms(FORMS);
    assert_eq!(session_forms.len(), 2);
    let encoded = session_forms[0]
        .encode(&fill)
        .expect("extract fields must be consumable by fill_form");
    let body = String::from_utf8(encoded.bytes).expect("utf8 body");
    for expected in [
        "user=alice",
        "csrf=tok-123",
        "remember=on",
        "country=fr",
        "bio=hello",
    ] {
        assert!(body.contains(expected), "missing {expected} in {body}");
    }
}

/// EXTRACT-12：links 按域名 / rel / 正则过滤生效；非法正则 → PARSE。
#[test]
fn extract_12_link_filters() {
    let opts = options();
    let base = Some(LINKS_BASE);
    let all = links(LINKS, base, &LinkFilter::default(), &opts);
    assert_valid(&all);
    assert!(all.ok, "{}", all.error);
    assert_eq!(all.data.as_ref().expect("links").observed, 6);
    assert_eq!(all.data.as_ref().expect("links").links.len(), 6);

    let by_domain = LinkFilter {
        domains: vec!["other.example".to_owned()],
        ..LinkFilter::default()
    };
    let env = links(LINKS, base, &by_domain, &opts);
    assert_valid(&env);
    assert!(env.ok);
    let data = env.data.expect("links");
    assert_eq!(data.links.len(), 1);
    assert!(data.links[0].href.contains("other.example"));
    assert_eq!(data.filtered_out, 5);

    let by_rel = LinkFilter {
        rel: vec!["nofollow".to_owned()],
        ..LinkFilter::default()
    };
    let env = links(LINKS, base, &by_rel, &opts);
    assert_eq!(env.data.expect("links").links.len(), 1);

    let by_pattern = LinkFilter {
        pattern: Some(r"page\.html$".to_owned()),
        ..LinkFilter::default()
    };
    let env = links(LINKS, base, &by_pattern, &opts);
    assert_eq!(env.data.expect("links").links.len(), 1);

    let internal = LinkFilter {
        internal_only: true,
        ..LinkFilter::default()
    };
    let env = links(LINKS, base, &internal, &opts);
    assert_eq!(env.data.expect("links").links.len(), 4);

    let external = LinkFilter {
        external_only: true,
        ..LinkFilter::default()
    };
    let env = links(LINKS, base, &external, &opts);
    assert_eq!(env.data.expect("links").links.len(), 2);

    let bad = LinkFilter {
        pattern: Some("(".to_owned()),
        ..LinkFilter::default()
    };
    let env = links(LINKS, base, &bad, &opts);
    assert_valid(&env);
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Parse);
}

/// EXTRACT-13：稳定性语义——同一选择器跨请求不保证命中同一元素。
#[test]
fn extract_13_stability_semantics() {
    let doc_a = "<ul><li class=\"item\">A-ONE</li><li class=\"item\">A-TWO</li></ul>";
    let doc_b = "<ul><li class=\"item\">B-ONE</li></ul>";
    let a = select_matches(doc_a, "li.item", &options())
        .data
        .expect("a");
    let b = select_matches(doc_b, "li.item", &options())
        .data
        .expect("b");
    assert_eq!(a.stability, STABILITY_NOTE);
    assert_eq!(b.stability, STABILITY_NOTE);
    assert!(STABILITY_NOTE.contains("not stable across requests"));
    assert_ne!(a.matches[0].text, b.matches[0].text);
    assert_ne!(a.matches.len(), b.matches.len());
}

/// EXTRACT-14：1MB HTML 抽取不超时且结果有界。
#[test]
fn extract_14_large_page_is_bounded() {
    let block = "<div class=\"bulk\">BULK-ITEM</div>";
    let scale = 1_048_576 / block.len() + 1;
    let big = block.repeat(scale);
    assert!(big.len() >= 1_048_576);
    let mut opts = options();
    opts.max_results = 10;
    let started = Instant::now();
    let env = select_matches(&big, "div.bulk", &opts);
    let wall = started.elapsed().as_millis();
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.code, ErrorCode::Limit);
    let result = env.data.expect("extract data");
    assert_eq!(result.matches.len(), 10);
    assert!(result.matches.len() <= opts.max_results);
    assert!(wall < 5_000, "1MB extract took {wall} ms");
}

/// EXTRACT-15：extract 结果与 PARSE-* 层断言一致。
#[test]
fn extract_15_matches_parse_layer() {
    let parse_links = extract_links(LINKS, Some(DOC_URL))
        .data
        .expect("parse links");
    let extract_links_data = links(LINKS, Some(DOC_URL), &LinkFilter::default(), &options())
        .data
        .expect("extract links")
        .links;
    assert_eq!(extract_links_data, parse_links);

    let parse_forms = extract_forms(FORMS, Some(DOC_URL))
        .data
        .expect("parse forms");
    let extract_forms_data = forms(FORMS, Some(DOC_URL), &options())
        .data
        .expect("extract forms");
    assert_eq!(extract_forms_data, parse_forms);

    let parse_tables = extract_tables(TABLES).data.expect("parse tables");
    let extract_tables_data = tables(TABLES, &options()).data.expect("extract tables");
    assert_eq!(extract_tables_data, parse_tables);
}

/// EXTRACT-16：结果不夹带无法审计的原始字节；二进制内容走 UNSUPPORTED。
#[test]
fn extract_16_no_raw_bytes() {
    let result = select_matches(PAGE, "li.item", &options())
        .data
        .expect("extract data");
    let json = serde_json::to_string(&result).expect("json");
    assert!(!json.contains("\\u0000"));
    assert!(!json.contains('\u{fffd}'));
    for hit in &result.matches {
        assert!(!hit.text.contains('\u{0}'));
        assert!(!hit.html.contains('\u{0}'));
    }

    let env = extract_body(&binary_body(ContentKind::Binary), "div", &options());
    assert_valid(&env);
    assert!(env.ok);
    assert_eq!(env.code, ErrorCode::Unsupported);
    assert!(env.meta.degraded_from.is_some());
    let data = env.data.expect("unsupported data");
    assert!(data.matches.is_empty());
    assert!(data.conclusion.contains("requires HTML"));
}

/// EXTRACT-17：灾难性输入有超时 / 复杂度上限，不导致 CPU 爆炸。
#[test]
fn extract_17_catastrophic_input_is_bounded() {
    let long_selector = "a".repeat(100_000);
    let started = Instant::now();
    let env = select_matches(PAGE, &long_selector, &options());
    let wall = started.elapsed().as_millis();
    assert_valid(&env);
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Blocked);
    assert!(wall < 1_000, "oversized selector took {wall} ms");

    let complex = ":not(".repeat(80);
    assert!(complex.len() <= options().max_selector_len);
    let env = select_matches(PAGE, &complex, &options());
    assert_valid(&env);
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Blocked);

    let mut deadline = options();
    deadline.max_elapsed_ms = 0;
    let env = select_matches(PAGE, "li.item", &deadline);
    assert_valid(&env);
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Timeout);

    let haystack = format!("{}b", "a".repeat(100_000));
    let search_opts = SearchOptions {
        regex: true,
        max_results: 10,
        ..SearchOptions::default()
    };
    let started = Instant::now();
    let env = search(&haystack, "(a+)+$", &search_opts);
    let wall = started.elapsed().as_millis();
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    assert!(env.data.expect("search data").matches.is_empty());
    assert!(wall < 2_000, "catastrophic regex took {wall} ms");
}

/// FORM-09：多表单页用 form_index 正确选定表单；越界是明确错误。
#[test]
fn form_09_multi_form_selection() {
    let env = forms(MULTI_FORM, None, &options());
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.data.expect("forms").len(), 2);

    let env = form_by_index(MULTI_FORM, None, 1);
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    let form = env.data.expect("form");
    assert_eq!(form.id.as_deref(), Some("second"));
    assert_eq!(form.method, "get");
    let names: Vec<&str> = form
        .fields
        .iter()
        .filter_map(|field| field.name.as_deref())
        .collect();
    assert_eq!(names, ["b", "c"]);

    let env = form_by_index(MULTI_FORM, None, 9);
    assert_valid(&env);
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Parse);
    assert!(env.error.contains("out of range"), "{}", env.error);
}

/// PARSE-12：选择器多命中的完整 extract 语义（结构化路径 + 上下文 + 预算）。
#[test]
fn parse_12_full_extract_semantics() {
    let mut opts = options();
    opts.context_chars = 12;
    let env = select_matches(PAGE, "p.lead", &opts);
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    let result = env.data.expect("extract data");
    assert_eq!(result.matches.len(), 2);

    let first = &result.matches[0];
    assert!(first.text.contains("ALPHAMARK"), "{}", first.text);
    assert!(
        first.before.contains("HEADERMARK"),
        "before={}",
        first.before
    );
    assert!(first.after.contains("Before"), "after={}", first.after);
    assert_eq!(first.path.sibling_index, 0);
    assert!(first.path.depth >= 3);

    let second = &result.matches[1];
    assert_eq!(second.path.sibling_index, 1);
    assert!(second.after.contains("GAMMAMARK"), "after={}", second.after);
}

/// A6：空结果 / 解析失败 / UNSUPPORTED / BLOCKED / LIMIT 五种码互不相同。
#[test]
fn extract_outcomes_have_distinct_codes() {
    let opts = options();
    let empty = select_matches(PAGE, "article.nope", &opts);
    let parse = select_matches(PAGE, "div[", &opts);
    let unsupported = extract_body(&binary_body(ContentKind::Binary), "div", &opts);
    let mut limited = options();
    limited.max_results = 1;
    let limit = select_matches(PAGE, "li.item", &limited);
    let blocked = select_matches(PAGE, &"a".repeat(100_000), &opts);

    for envelope in [&empty, &parse, &unsupported, &limit, &blocked] {
        assert_valid(envelope);
    }
    assert_eq!(empty.code, ErrorCode::Ok);
    assert!(empty.ok);
    assert_eq!(parse.code, ErrorCode::Parse);
    assert!(!parse.ok);
    assert_eq!(unsupported.code, ErrorCode::Unsupported);
    assert!(unsupported.ok);
    assert!(unsupported.meta.degraded_from.is_some());
    assert_eq!(limit.code, ErrorCode::Limit);
    assert!(limit.ok);
    assert!(limit.meta.degraded_from.is_some());
    assert_eq!(blocked.code, ErrorCode::Blocked);
    assert!(!blocked.ok);

    let codes = [
        empty.code,
        parse.code,
        unsupported.code,
        limit.code,
        blocked.code,
    ];
    for (index, left) in codes.iter().enumerate() {
        for right in &codes[index + 1..] {
            assert_ne!(left, right, "codes must be distinct");
        }
    }
}
