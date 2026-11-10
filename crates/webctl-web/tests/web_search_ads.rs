//! 去广告离线回归（0.2.3 M1，议题 #221）。
//!
//! 三份 fixture 都是**手写合成**的脱敏 SERP，不包含真实抓取数据，也不访问网络。
//! 每份固定包含：1 条自然结果 + 1 条低置信广告标签 + 1 条高置信跳转链。
//!
//! 验收口径：
//! - 高置信样本被剔除，`filtered_ads` 计数与高置信样本数一致；
//! - 低置信样本保留且 `is_ad == true`；
//! - 自然结果数量不变（误杀 = 0）。

use webctl_web::web_search::{SearchHit, SearchReport, baidu, bing, so360};

const BING: &str = include_str!("fixtures/web_search/bing.html");
const BAIDU: &str = include_str!("fixtures/web_search/baidu.html");
const SO360: &str = include_str!("fixtures/web_search/so360.html");

const HIGH_MARKER: &str = "ads.example.org";

fn assert_kept_results(results: &[SearchHit]) {
    assert_eq!(results.len(), 2, "高置信广告之外应保留 2 条");
    assert_eq!(results[0].rank, 1);
    assert_eq!(results[0].url, "https://example.com/docs/rust");
    assert!(!results[0].is_ad, "自然结果不得被误标为广告");
    assert_eq!(results[1].rank, 2);
    assert_eq!(results[1].url, "https://example.net/course");
    assert!(results[1].is_ad, "低置信标签样本应保留并打标");
    assert_eq!(
        results.iter().filter(|hit| !hit.is_ad).count(),
        1,
        "自然结果数量必须保持不变（误杀 = 0）"
    );
    assert!(
        results.iter().all(|hit| !hit.url.contains(HIGH_MARKER)),
        "高置信跳转链的真实目标不得出现在结果里"
    );
    assert!(
        results.iter().all(|hit| hit.title != "特惠云主机"),
        "高置信广告标题不得出现"
    );
}

#[test]
fn bing_high_confidence_redirect_is_filtered_and_counted() {
    let serp = bing::parse_serp(BING, 10);
    assert_eq!(serp.filtered_ads, 1, "应剔除且只剔除 1 条高置信广告");
    assert_kept_results(&serp.results);
}

#[test]
fn baidu_high_confidence_redirect_is_filtered_and_counted() {
    let serp = baidu::parse_serp(BAIDU, 10);
    assert_eq!(serp.filtered_ads, 1, "应剔除且只剔除 1 条高置信广告");
    assert_kept_results(&serp.results);
}

#[test]
fn so360_high_confidence_redirect_is_filtered_and_counted() {
    let serp = so360::parse_serp(SO360, 10);
    assert_eq!(serp.filtered_ads, 1, "应剔除且只剔除 1 条高置信广告");
    assert_kept_results(&serp.results);
}

#[test]
fn natural_redirect_style_is_not_mistaken_for_an_ad() {
    // Baidu 的 /link 与 360 的 /link（小写）是自然结果的跳转样式，不能按广告处理。
    let baidu = baidu::parse_serp(BAIDU, 10);
    assert!(baidu.results.iter().any(|hit| hit.engine == "baidu"));
    assert_eq!(baidu.filtered_ads, 1);

    let so360 = so360::parse_serp(SO360, 10);
    assert!(so360.results.iter().any(|hit| hit.engine == "360"));
    assert_eq!(so360.filtered_ads, 1);
}

#[test]
fn filtered_ads_and_is_ad_are_visible_in_the_report_json() {
    let serp = baidu::parse_serp(BAIDU, 10);
    let report = SearchReport {
        query: "rust".to_owned(),
        results: serp.results,
        filtered_ads: serp.filtered_ads,
        applied_operators: Vec::new(),
        engines: Vec::new(),
        llm_summary: None,
        related: Vec::new(),
        notes: Vec::new(),
    };
    let value = serde_json::to_value(&report).expect("SearchReport 必须可序列化");
    assert_eq!(
        value["filtered_ads"], 1,
        "filtered_ads 必须在输出 envelope 中可见，不能静默删除"
    );
    assert_eq!(value["results"][1]["is_ad"], true, "低置信广告必须打标");
    assert!(
        value["results"][0].get("is_ad").is_none(),
        "非广告默认 false，不应写进 JSON"
    );
}

#[test]
fn max_results_counts_natural_results_only() {
    // 高置信广告先剔除，再按自然结果数量截断；不能因为广告占坑少返回。
    let serp = bing::parse_serp(BING, 1);
    assert_eq!(serp.filtered_ads, 1);
    assert_eq!(serp.results.len(), 1);
    assert_eq!(serp.results[0].url, "https://example.com/docs/rust");
}
