//! 排除操作符（0.2.3 M2，议题 #222）：规范化、URL 透传、生效校验。
//!
//! 全部离线：fixture 是仓库内手写合成的脱敏 SERP，不发起任何网络请求。

use webctl_web::web_search::engine::{normalize_exclude, operators_ignored};
use webctl_web::web_search::{SearchHit, baidu, bing, so360};

const BING: &str = include_str!("fixtures/web_search/bing.html");
const BAIDU: &str = include_str!("fixtures/web_search/baidu.html");
const SO360: &str = include_str!("fixtures/web_search/so360.html");

fn hit(rank: u32, url: &str, title: &str, snippet: &str) -> SearchHit {
    SearchHit {
        rank,
        engine: "bing".to_owned(),
        title: title.to_owned(),
        url: url.to_owned(),
        display_url: url.to_owned(),
        snippet: snippet.to_owned(),
        engines: vec!["bing".to_owned()],
        is_ad: false,
        same_domain_count: None,
        snippet_truncated: false,
    }
}

#[test]
fn normalize_exclude_prefixes_drops_blanks_and_dedupes() {
    let items = vec![
        "tutorial".to_owned(),
        "  -site:csdn.example  ".to_owned(),
        String::new(),
        "   ".to_owned(),
        "-tutorial".to_owned(),
        "site:csdn.example".to_owned(),
    ];
    assert_eq!(
        normalize_exclude(&items),
        vec!["-tutorial".to_owned(), "-site:csdn.example".to_owned()]
    );
    assert!(normalize_exclude(&[]).is_empty());
}

#[test]
fn every_engine_url_carries_encoded_operators() {
    let exclude = vec!["site:csdn.example".to_owned(), "tutorial".to_owned()];
    let urls = [
        bing::search_url(bing::DEFAULT_BASE, "rust & web", 1, 10, &exclude),
        baidu::search_url(baidu::DEFAULT_BASE, "rust & web", 1, 10, &exclude),
        so360::search_url(so360::DEFAULT_BASE, "rust & web", 1, 10, &exclude),
    ];
    for url in urls {
        assert!(
            url.contains("rust+%26+web+-site%3Acsdn.example+-tutorial"),
            "查询串编码或操作符拼接不对：{url}"
        );
    }
}

#[test]
fn empty_exclude_keeps_the_url_unchanged() {
    // 用保留域名 as base，避免测试源码出现真实外站（L0 车道扫描）。
    assert_eq!(
        bing::search_url("https://serp.example/search", "rust web", 1, 10, &[]),
        "https://serp.example/search?q=rust+web&first=1&count=10"
    );
}

#[test]
fn ignored_site_is_detected_from_result_host() {
    let results = vec![hit(
        1,
        "https://blog.csdn.example/post",
        "Rust 教程",
        "CSDN 博客",
    )];
    assert!(operators_ignored(
        &results,
        &["-site:csdn.example".to_owned()]
    ));
    assert!(operators_ignored(
        &results,
        &["site:csdn.example".to_owned()]
    ));
    assert!(!operators_ignored(
        &results,
        &["-site:example.com".to_owned()]
    ));
    assert!(!operators_ignored(
        &results,
        &["-site:csdn.example.evil.test".to_owned()]
    ));
}

#[test]
fn ignored_term_is_detected_from_title_or_snippet() {
    let by_title = vec![hit(1, "https://example.com/a", "Rust Tutorial", "文档")];
    let by_snippet = vec![hit(
        2,
        "https://example.com/b",
        "Rust 文档",
        "a tutorial for rust",
    )];
    assert!(operators_ignored(&by_title, &["-tutorial".to_owned()]));
    assert!(operators_ignored(&by_snippet, &["tutorial".to_owned()]));
    assert!(!operators_ignored(&by_title, &["-python".to_owned()]));
}

#[test]
fn no_operators_or_no_results_is_not_ignored() {
    let results = vec![hit(1, "https://blog.csdn.example/post", "Rust", "CSDN")];
    assert!(!operators_ignored(&results, &[]));
    assert!(!operators_ignored(&[], &["-site:csdn.example".to_owned()]));
}

/// 既有 fixture 解析出的结果不含 csdn.net；用真实解析结果跑组合断言（不发请求）。
#[test]
fn parsed_fixtures_do_not_contain_the_excluded_site() {
    let parsed = [
        bing::parse_serp(BING, 10).results,
        baidu::parse_serp(BAIDU, 10).results,
        so360::parse_serp(SO360, 10).results,
    ];
    for results in &parsed {
        assert!(!operators_ignored(
            results,
            &["-site:csdn.example".to_owned()]
        ));
        // 同一份结果里命中自然结果中的词，才报"忽略"——证明校验看的是真实解析结果。
        assert!(operators_ignored(results, &["-rust".to_owned()]));
    }
}
