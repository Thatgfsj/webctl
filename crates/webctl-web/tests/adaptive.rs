//! Spike #195：自适应选择器跟踪的离线验收。
//!
//! 全部用例只读本地 fixture，不联网、不启动子进程。
//! 断言的核心不是"能找回来"，而是"找回来时必须如实标注相似度、置信度与候选证据"。

use serde_json::json;
use webctl_core::{Envelope, ErrorCode};
use webctl_web::extract::adaptive::{AdaptiveOptions, MatchedBy, TrackStatus, track};
use webctl_web::extract::{ExtractOptions, select_matches};

const NEWS_V1: &str = include_str!("fixtures/adaptive/news_v1.html");
const NEWS_V2: &str = include_str!("fixtures/adaptive/news_v2.html");
const NEWS_V3: &str = include_str!("fixtures/adaptive/news_v3_radical.html");
const DUPLICATE_V1: &str = include_str!("fixtures/adaptive/duplicate_v1.html");
const DUPLICATE_V2: &str = include_str!("fixtures/adaptive/duplicate_v2.html");
const THIN_V1: &str = include_str!("fixtures/adaptive/thin_v1.html");
const THIN_V2: &str = include_str!("fixtures/adaptive/thin_v2.html");

fn options() -> AdaptiveOptions {
    AdaptiveOptions::default()
}

fn assert_valid<T: std::fmt::Debug>(envelope: &Envelope<T>) {
    assert_eq!(
        envelope.validate(),
        Ok(()),
        "envelope invariants: {envelope:?}"
    );
}

/// ADAPT-01：精确选择器仍然命中时走 exact，不跑相似度、不标 similarity。
#[test]
fn exact_hit_is_not_relabelled() {
    let env = track(NEWS_V1, "#post-1 .post-title a", NEWS_V1, &options());
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.code, ErrorCode::Ok);
    let data = env.data.expect("data");
    assert_eq!(data.status, TrackStatus::Exact);
    assert_eq!(data.matched_by, MatchedBy::Selector);
    assert_eq!(data.confidence, 1.0);
    assert!(!data.requires_confirmation);
    assert_eq!(data.candidates.len(), 1);
    assert!(data.candidates[0].text.contains("riverside park"));
    assert_eq!(
        data.candidates[0].evidence,
        vec!["exact_selector_match".to_owned()]
    );
}

/// ADAPT-02：改版后精确选择器零命中，唯一候选标 similarity + confidence，
/// 并要求调用方确认；输出 JSON 里的 matched_by 不是 selector。
#[test]
fn redesigned_page_relocates_with_similarity_label() {
    let env = track(NEWS_V1, "#post-1 .post-title a", NEWS_V2, &options());
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    let value = serde_json::to_value(&env).expect("serialize");
    let data = env.data.expect("data");
    assert_eq!(data.status, TrackStatus::Relocated);
    assert_eq!(data.matched_by, MatchedBy::Similarity);
    assert!(data.requires_confirmation);
    assert!(data.confidence >= options().min_confidence);
    assert!(data.confidence < 1.0);
    assert!(
        data.fingerprint
            .as_ref()
            .is_some_and(|fp| !fp.text.is_empty())
    );
    let top = data.candidates.first().expect("candidate");
    assert!(top.text.contains("riverside park"), "{}", top.text);
    assert!(
        top.evidence
            .iter()
            .any(|line| line.starts_with("text_dice=")),
        "{:?}",
        top.evidence
    );

    assert_eq!(value["data"]["matched_by"], json!("similarity"));
    assert_eq!(value["data"]["requires_confirmation"], json!(true));
    assert!(value["data"]["confidence"].as_f64().is_some());
}

/// ADAPT-03：同一站点改版前后两版 HTML 的命中率。
///
/// 6 个锚点（5 条标题链接 + 1 段正文）在 v2 里全部被重定位；v2 同时做了
/// 标签替换、class 重命名、包裹层插入与条目换序，避免"只是换了 class"的弱样本。
#[test]
fn relocation_hit_rate_on_same_site_redesign() {
    let cases = [
        ("#post-1 .post-title a", "City opens new riverside park"),
        ("#post-2 .post-title a", "Council approves night bus plan"),
        ("#post-3 .post-title a", "Library extends weekend hours"),
        (
            "#post-4 .post-title a",
            "Farmers market moves to the square",
        ),
        ("#post-5 .post-title a", "Harbour clean-up draws volunteers"),
        (
            "#post-2 .post-body p",
            "Night buses will run every thirty minutes",
        ),
    ];
    let mut hits = 0usize;
    for (selector, expected) in cases {
        let env = track(NEWS_V1, selector, NEWS_V2, &options());
        assert_valid(&env);
        assert!(env.ok, "{selector}: {}", env.error);
        let data = env.data.expect("data");
        assert_eq!(data.status, TrackStatus::Relocated, "{selector}");
        assert_eq!(data.matched_by, MatchedBy::Similarity, "{selector}");
        assert!(data.requires_confirmation, "{selector}");
        let top = data.candidates.first().expect("candidate");
        assert!(
            top.text.contains(expected),
            "{selector}: top={:?} evidence={:?}",
            top.text,
            top.evidence
        );
        assert!(
            top.confidence >= options().min_confidence && top.confidence < 1.0,
            "{selector}: confidence={}",
            top.confidence
        );
        hits += 1;
    }
    println!("hit rate: {hits}/{}", cases.len());
    assert_eq!(hits, cases.len());
}

/// ADAPT-04：小范围动态文本（日期变化）仍能重定位，但置信度低于原文不变的标题。
#[test]
fn dynamic_byline_text_still_relocates_with_lower_confidence() {
    let title = track(NEWS_V1, "#post-3 .post-title a", NEWS_V2, &options())
        .data
        .expect("title data");
    let byline = track(NEWS_V1, "#post-3 .post-meta", NEWS_V2, &options())
        .data
        .expect("byline data");
    assert_eq!(byline.status, TrackStatus::Relocated, "{byline:?}");
    assert!(byline.requires_confirmation);
    assert!(byline.confidence >= options().min_confidence);
    assert!(
        byline.confidence <= title.confidence,
        "date change should not score higher than identical text: {} > {}",
        byline.confidence,
        title.confidence
    );
}

/// ADAPT-05：内容与结构同时大改时返回 not_found，不硬塞一个候选。
#[test]
fn radical_change_is_reported_not_found() {
    let env = track(NEWS_V1, "#post-1 .post-title a", NEWS_V3, &options());
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.code, ErrorCode::Ok);
    let data = env.data.expect("data");
    assert_eq!(data.status, TrackStatus::NotFound);
    assert_eq!(data.matched_by, MatchedBy::None);
    assert!(!data.requires_confirmation);
    assert!(data.confidence < options().min_confidence);
    assert!(
        data.candidates
            .iter()
            .all(|candidate| !candidate.text.contains("riverside park")),
        "radical fixture must not contain the old headline: {:?}",
        data.candidates.iter().map(|c| &c.text).collect::<Vec<_>>()
    );
}

/// ADAPT-06：重复 DOM（桌面端 + 移动端同一份内容）是歧义，不自动二选一。
#[test]
fn duplicate_dom_is_ambiguous_not_relocated() {
    let env = track(
        DUPLICATE_V1,
        "#weekly-card .headline",
        DUPLICATE_V2,
        &options(),
    );
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    let data = env.data.expect("data");
    assert_eq!(data.status, TrackStatus::Ambiguous);
    assert_eq!(data.matched_by, MatchedBy::Similarity);
    assert!(data.requires_confirmation);
    assert!(data.candidates.len() >= 2);
    assert_eq!(
        data.candidates[0].confidence, data.candidates[1].confidence,
        "identical duplicate candidates must tie: {:?}",
        data.candidates
    );
    assert!(data.conclusion.contains("refusing to pick one"));
}

/// ADAPT-07：指纹只有 tag/祖先、没有文本/属性时拒绝匹配。
#[test]
fn insufficient_fingerprint_is_refused() {
    let env = track(THIN_V1, "div", THIN_V2, &options());
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    let data = env.data.expect("data");
    assert_eq!(data.status, TrackStatus::InsufficientFingerprint);
    assert_eq!(data.matched_by, MatchedBy::None);
    assert!(data.fingerprint.is_some());
    assert!(data.conclusion.contains("refusing to match"));
}

/// ADAPT-08：扫描上限被触发时返回 LIMIT + degraded_from，而不是悄悄截断。
#[test]
fn scan_cap_is_visible_as_limit_degradation() {
    let options = AdaptiveOptions {
        max_scan_nodes: 1,
        ..AdaptiveOptions::default()
    };
    let env = track(NEWS_V1, "#post-1 .post-title a", NEWS_V2, &options);
    assert_valid(&env);
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.code, ErrorCode::Limit);
    assert_eq!(
        env.meta.degraded_from.as_deref(),
        Some("adaptive-candidate-cap")
    );
    let actionable = env.meta.actionable.expect("actionable must be attached");
    assert_eq!(actionable.reason, "adaptive-candidate-cap-reached");
    let data = env.data.expect("data");
    assert!(data.truncated);
    assert!(data.scanned_nodes <= options.max_scan_nodes);
}

/// ADAPT-09：墙钟预算为 0 时，相似度阶段超时；不返回空结果冒充成功。
#[test]
fn zero_budget_times_out_similarity_phase() {
    let options = AdaptiveOptions {
        max_elapsed_ms: 0,
        ..AdaptiveOptions::default()
    };
    let env = track(NEWS_V1, "#post-1 .post-title a", NEWS_V2, &options);
    assert_valid(&env);
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Timeout);
    assert!(env.data.is_none());
    assert!(env.error.contains("budget"), "{}", env.error);
}

/// ADAPT-11：spike 不改默认 extract：旧选择器在 v2 上仍是合法零命中。
#[test]
fn default_extract_behaviour_is_untouched() {
    let old = select_matches(NEWS_V2, "#post-1 .post-title a", &ExtractOptions::default());
    assert_valid(&old);
    assert!(old.ok, "{}", old.error);
    assert_eq!(old.code, ErrorCode::Ok);
    assert!(old.data.expect("data").matches.is_empty());

    let current = select_matches(
        NEWS_V2,
        ".story-card .headline a",
        &ExtractOptions::default(),
    );
    assert!(current.ok);
    assert_eq!(current.data.expect("data").matches.len(), 5);
}
