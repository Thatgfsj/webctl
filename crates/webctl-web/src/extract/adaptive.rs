//! 自适应选择器跟踪（Spike #195，研究原型，未接入默认抽取）。
//!
//! 给上一版 HTML + 选择器建立轻量指纹，在当前 HTML 精确零命中后按相似度返回候选；
//! 结果永远标 matched_by/confidence/evidence，且 requires_confirmation=true，
//! 绝不写回 ExtractResult，也不让零命中冒充成功。
//!
//! 指纹：tag / 稳定 id-class / 自身文本 / 邻近文本 / 祖先路径 / 深度与同级索引。
//! 相似度：6 分量加权平均（tag/id/class/text/near/path），文本用字符二元组 Dice，
//! 路径用祖先 tag 的 LCS；无模型、无网络、无新依赖。
//!
//! 研究原型，不是稳定 API。

use std::collections::BTreeSet;
use std::time::Instant;

use scraper::{ElementRef, Html, Node};
use serde::{Deserialize, Serialize};
use webctl_core::{Envelope, ErrorCode, Trust};

use super::{ElementPath, ExtractOptions, context, head_chars, path};
use crate::parse::elapsed_ms;

const W_TAG: f32 = 0.15;
const W_ID: f32 = 0.10;
const W_CLASS: f32 = 0.15;
const W_TEXT: f32 = 0.35;
const W_NEAR: f32 = 0.10;
const W_PATH: f32 = 0.15;

/// 自适应跟踪的选项。默认值刻意保守：宁可说"找不到"，也不拖慢或猜错。
#[derive(Debug, Clone, PartialEq)]
pub struct AdaptiveOptions {
    pub min_confidence: f32,
    pub min_margin: f32,
    pub top_k: usize,
    pub max_scan_nodes: usize,
    pub max_candidates: usize,
    pub max_elapsed_ms: u64,
    pub max_text_chars: usize,
    pub max_near_chars: usize,
}

impl Default for AdaptiveOptions {
    fn default() -> Self {
        Self {
            min_confidence: 0.60,
            min_margin: 0.06,
            top_k: 3,
            max_scan_nodes: 5_000,
            max_candidates: 200,
            max_elapsed_ms: 50,
            max_text_chars: 200,
            max_near_chars: 80,
        }
    }
}

/// 一个元素的轻量指纹；可序列化，便于调用方跨请求保存后回传。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementFingerprint {
    pub tag: String,
    pub stable_id: Option<String>,
    pub stable_classes: Vec<String>,
    pub text: String,
    pub before: String,
    pub after: String,
    pub ancestor_tags: Vec<String>,
    pub depth: usize,
    pub sibling_index: usize,
}

impl ElementFingerprint {
    fn informative_features(&self) -> usize {
        usize::from(self.stable_id.is_some())
            + usize::from(!self.stable_classes.is_empty())
            + usize::from(!self.text.is_empty())
            + usize::from(!self.before.is_empty() || !self.after.is_empty())
            + usize::from(!self.ancestor_tags.is_empty())
    }
}

/// 跟踪结果的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackStatus {
    Exact,
    Relocated,
    Ambiguous,
    NotFound,
    InsufficientFingerprint,
}

/// 结果的匹配方式；相似度结果永远不会伪装成 selector。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchedBy {
    Selector,
    Similarity,
    None,
}

/// 一条候选及其可核对的证据。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackCandidate {
    pub index: usize,
    pub tag: String,
    pub path: ElementPath,
    pub text: String,
    pub before: String,
    pub after: String,
    pub confidence: f32,
    pub evidence: Vec<String>,
}

/// 一次自适应跟踪的结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackResult {
    pub selector: String,
    pub status: TrackStatus,
    pub matched_by: MatchedBy,
    pub confidence: f32,
    pub requires_confirmation: bool,
    pub fingerprint: Option<ElementFingerprint>,
    pub candidates: Vec<TrackCandidate>,
    pub scanned_nodes: usize,
    pub considered_candidates: usize,
    pub truncated: bool,
    pub conclusion: String,
}

/// 用上一版 HTML + 选择器建立指纹，在当前版 HTML 中精确命中或按相似度定位。
///
/// 语义：
/// - 选择器在当前文档仍有命中 → exact，matched_by=selector，不跑相似度；
/// - 当前零命中 → 在预算内扫描候选，返回 relocated / ambiguous / not_found；
/// - 指纹特征 < 2 → insufficient_fingerprint（不猜）；
/// - 扫描或候选截断 → code=LIMIT + meta.degraded_from="adaptive-candidate-cap"；
/// - 超出墙钟预算 → code=TIMEOUT。
///
/// 本函数**不修改**任何默认抽取行为，也不写入任何全局状态。
pub fn track(
    previous_html: &str,
    selector: &str,
    current_html: &str,
    options: &AdaptiveOptions,
) -> Envelope<TrackResult> {
    let started = Instant::now();
    if let Some((code, message)) = super::guard_selector(selector, &ExtractOptions::default()) {
        return Envelope::fail(code, message, elapsed_ms(started));
    }
    let css = match crate::parse::html::compile_selector(selector) {
        Ok(css) => css,
        Err(message) => return Envelope::fail(ErrorCode::Parse, message, elapsed_ms(started)),
    };
    let previous = crate::parse::html::parse_document(previous_html);
    let Some(anchor) = previous.select(&css).next() else {
        let mut result = empty_result(selector, TrackStatus::NotFound);
        result.conclusion = format!(
            "selector {selector:?} matched 0 elements in the previous snapshot; there is no anchor fingerprint to track"
        );
        return Envelope::ok(result, Trust::UntrustedPageContent, elapsed_ms(started));
    };
    let fingerprint = fingerprint_of(&anchor, options);
    if fingerprint.informative_features() < 2 {
        let features = fingerprint.informative_features();
        let mut result = empty_result(selector, TrackStatus::InsufficientFingerprint);
        result.fingerprint = Some(fingerprint);
        result.conclusion = format!(
            "anchor fingerprint has only {features} informative feature(s); need at least 2 (id/class/text/neighbor/path); refusing to match"
        );
        return Envelope::ok(result, Trust::UntrustedPageContent, elapsed_ms(started));
    }

    let current = crate::parse::html::parse_document(current_html);
    let exact_total = current.select(&css).count();
    if exact_total > 0 {
        let top_k = options.top_k.max(1);
        let mut result = empty_result(selector, TrackStatus::Exact);
        result.matched_by = MatchedBy::Selector;
        result.confidence = 1.0;
        result.fingerprint = Some(fingerprint);
        result.candidates = current
            .select(&css)
            .take(top_k)
            .enumerate()
            .map(|(index, element)| {
                let mut candidate = candidate_of(&element, 1.0, options);
                candidate.index = index;
                candidate.evidence = vec!["exact_selector_match".to_owned()];
                candidate
            })
            .collect();
        result.conclusion = format!(
            "selector {selector:?} matched {exact_total} element(s) in the current document; no similarity fallback was used"
        );
        return Envelope::ok(result, Trust::UntrustedPageContent, elapsed_ms(started));
    }

    if elapsed_ms(started) >= options.max_elapsed_ms {
        return timeout(started, options.max_elapsed_ms);
    }
    let scan = match scan_candidates(&current, &fingerprint, options, started) {
        Ok(scan) => scan,
        Err(()) => return timeout(started, options.max_elapsed_ms),
    };
    let mut candidates = scan.candidates;
    let top = candidates
        .first()
        .map_or(0.0, |candidate| candidate.confidence);
    let second = candidates
        .get(1)
        .map_or(0.0, |candidate| candidate.confidence);
    let margin = top - second;
    let min_confidence = options.min_confidence.clamp(0.0, 1.0);
    let min_margin = options.min_margin.max(0.0);

    let (status, matched_by, requires_confirmation, conclusion) = if top < min_confidence {
        (
            TrackStatus::NotFound,
            MatchedBy::None,
            false,
            format!(
                "selector {selector:?} matched 0 elements; best similarity {top:.3} is below min_confidence {min_confidence:.3}; no relocation"
            ),
        )
    } else if margin < min_margin {
        (
            TrackStatus::Ambiguous,
            MatchedBy::Similarity,
            true,
            format!(
                "selector {selector:?} matched 0 elements; top two candidates differ by {margin:.3} < min_margin {min_margin:.3}; refusing to pick one, inspect candidates manually"
            ),
        )
    } else {
        (
            TrackStatus::Relocated,
            MatchedBy::Similarity,
            true,
            format!(
                "selector {selector:?} matched 0 elements; best similarity candidate is <{}> at confidence {top:.3} (margin {margin:.3}); matched_by=similarity and requires_confirmation=true, this is not an exact hit",
                candidates
                    .first()
                    .map_or("?", |candidate| candidate.tag.as_str())
            ),
        )
    };

    let mut result = empty_result(selector, status);
    result.matched_by = matched_by;
    result.confidence = top;
    result.requires_confirmation = requires_confirmation;
    result.fingerprint = Some(fingerprint);
    result.scanned_nodes = scan.scanned_nodes;
    result.considered_candidates = scan.considered_candidates;
    result.truncated = scan.truncated;
    result.conclusion = conclusion;
    candidates.truncate(options.top_k.max(1));
    for (index, candidate) in candidates.iter_mut().enumerate() {
        candidate.index = index;
    }
    result.candidates = candidates;

    if result.truncated {
        Envelope::degraded(
            result,
            "adaptive-candidate-cap",
            ErrorCode::Limit,
            elapsed_ms(started),
        )
    } else {
        Envelope::ok(result, Trust::UntrustedPageContent, elapsed_ms(started))
    }
}

fn empty_result(selector: &str, status: TrackStatus) -> TrackResult {
    TrackResult {
        selector: selector.to_owned(),
        status,
        matched_by: MatchedBy::None,
        confidence: 0.0,
        requires_confirmation: false,
        fingerprint: None,
        candidates: Vec::new(),
        scanned_nodes: 0,
        considered_candidates: 0,
        truncated: false,
        conclusion: String::new(),
    }
}

fn timeout<T>(started: Instant, budget_ms: u64) -> Envelope<T> {
    Envelope::fail(
        ErrorCode::Timeout,
        format!("adaptive tracking exceeded wall-clock budget of {budget_ms} ms"),
        elapsed_ms(started),
    )
}

struct ScanOutcome {
    candidates: Vec<TrackCandidate>,
    scanned_nodes: usize,
    considered_candidates: usize,
    truncated: bool,
}

fn scan_candidates(
    document: &Html,
    fingerprint: &ElementFingerprint,
    options: &AdaptiveOptions,
    started: Instant,
) -> Result<ScanOutcome, ()> {
    let mut candidates = Vec::new();
    let mut scanned_nodes = 0usize;
    let mut considered_candidates = 0usize;
    let mut truncated = false;
    for node in document.tree.nodes() {
        let Some(element) = ElementRef::wrap(node) else {
            continue;
        };
        if element.parent().is_none() {
            continue;
        }
        let tag = element.value().name();
        if matches!(tag, "script" | "style" | "template" | "head") {
            continue;
        }
        if scanned_nodes >= options.max_scan_nodes {
            truncated = true;
            break;
        }
        scanned_nodes += 1;
        if scanned_nodes.is_multiple_of(128) && elapsed_ms(started) >= options.max_elapsed_ms {
            return Err(());
        }
        if !cheap_candidate(&element, fingerprint) {
            continue;
        }
        if considered_candidates >= options.max_candidates {
            truncated = true;
            break;
        }
        if elapsed_ms(started) >= options.max_elapsed_ms {
            return Err(());
        }
        considered_candidates += 1;
        candidates.push(score_candidate(&element, fingerprint, options));
    }
    if elapsed_ms(started) >= options.max_elapsed_ms {
        return Err(());
    }
    candidates.sort_by(|left, right| {
        right
            .confidence
            .total_cmp(&left.confidence)
            .then(left.index.cmp(&right.index))
    });
    Ok(ScanOutcome {
        candidates,
        scanned_nodes,
        considered_candidates,
        truncated,
    })
}

fn cheap_candidate(element: &ElementRef<'_>, fingerprint: &ElementFingerprint) -> bool {
    if element.value().name() == fingerprint.tag.as_str() {
        return true;
    }
    if let Some(id) = stable_id(element)
        && fingerprint.stable_id.as_deref() == Some(id.as_str())
    {
        return true;
    }
    let classes = stable_classes(element);
    if classes
        .iter()
        .any(|class| fingerprint.stable_classes.contains(class))
    {
        return true;
    }
    let direct = normalize_for_match(&direct_text(element));
    let anchor = normalize_for_match(&fingerprint.text);
    if !direct.is_empty()
        && !anchor.is_empty()
        && (direct.contains(&anchor) || anchor.contains(&direct) || dice(&direct, &anchor) >= 0.45)
    {
        return true;
    }
    false
}

fn score_candidate(
    element: &ElementRef<'_>,
    fingerprint: &ElementFingerprint,
    options: &AdaptiveOptions,
) -> TrackCandidate {
    let path = path::element_path(element);
    let tag = path.tag.clone();
    let id = stable_id(element);
    let classes = stable_classes(element);
    let text = head_chars(
        &crate::parse::html::collapse_ws(&context::visible_text(element)),
        options.max_text_chars,
    );
    let (before, after) = near_text(element, options.max_near_chars);
    let mut weighted = 0.0f32;
    let mut total = 0.0f32;
    let mut evidence = Vec::new();

    total += W_TAG;
    if tag == fingerprint.tag {
        weighted += W_TAG;
        evidence.push("tag=same".to_owned());
    } else {
        evidence.push(format!("tag={tag}"));
    }

    if let Some(anchor_id) = fingerprint.stable_id.as_deref() {
        total += W_ID;
        if id.as_deref() == Some(anchor_id) {
            weighted += W_ID;
            evidence.push("id=same".to_owned());
        } else {
            evidence.push("id=changed".to_owned());
        }
    }

    if !fingerprint.stable_classes.is_empty() {
        total += W_CLASS;
        let overlap = classes
            .iter()
            .filter(|class| fingerprint.stable_classes.contains(*class))
            .count();
        let union = fingerprint.stable_classes.len() + classes.len() - overlap;
        let jaccard = if union == 0 {
            0.0
        } else {
            overlap as f32 / union as f32
        };
        weighted += W_CLASS * jaccard;
        evidence.push(format!("class_jaccard={jaccard:.2}"));
    }

    if !fingerprint.text.is_empty() {
        total += W_TEXT;
        let similarity = dice(
            &normalize_for_match(&fingerprint.text),
            &normalize_for_match(&text),
        );
        weighted += W_TEXT * similarity;
        evidence.push(format!("text_dice={similarity:.2}"));
    }

    if !fingerprint.before.is_empty() || !fingerprint.after.is_empty() {
        total += W_NEAR;
        let similarity = dice(
            &normalize_for_match(&format!("{} {}", fingerprint.before, fingerprint.after)),
            &normalize_for_match(&format!("{before} {after}")),
        );
        weighted += W_NEAR * similarity;
        evidence.push(format!("near_dice={similarity:.2}"));
    }

    if !fingerprint.ancestor_tags.is_empty() {
        total += W_PATH;
        let similarity = path_similarity(fingerprint, &path);
        weighted += W_PATH * similarity;
        evidence.push(format!("path_sim={similarity:.2}"));
    }

    let confidence = if total > 0.0 {
        (weighted / total * 1000.0).round() / 1000.0
    } else {
        0.0
    };
    TrackCandidate {
        index: 0,
        tag,
        path,
        text,
        before,
        after,
        confidence,
        evidence,
    }
}

fn candidate_of(
    element: &ElementRef<'_>,
    confidence: f32,
    options: &AdaptiveOptions,
) -> TrackCandidate {
    let path = path::element_path(element);
    let (before, after) = near_text(element, options.max_near_chars);
    TrackCandidate {
        index: 0,
        tag: path.tag.clone(),
        path,
        text: head_chars(
            &crate::parse::html::collapse_ws(&context::visible_text(element)),
            options.max_text_chars,
        ),
        before,
        after,
        confidence,
        evidence: Vec::new(),
    }
}

fn fingerprint_of(element: &ElementRef<'_>, options: &AdaptiveOptions) -> ElementFingerprint {
    let path = path::element_path(element);
    let (before, after) = near_text(element, options.max_near_chars);
    ElementFingerprint {
        tag: path.tag.clone(),
        stable_id: stable_id(element),
        stable_classes: stable_classes(element),
        text: head_chars(
            &crate::parse::html::collapse_ws(&context::visible_text(element)),
            options.max_text_chars,
        ),
        before,
        after,
        ancestor_tags: path
            .ancestors
            .iter()
            .map(|segment| segment.tag.clone())
            .collect(),
        depth: path.depth,
        sibling_index: path.sibling_index,
    }
}

fn near_text(element: &ElementRef<'_>, max_chars: usize) -> (String, String) {
    let before = element
        .prev_siblings()
        .filter_map(ElementRef::wrap)
        .find(|sibling| !context::skips_text(sibling.value().name()))
        .map(|sibling| crate::parse::html::collapse_ws(&context::visible_text(&sibling)))
        .unwrap_or_default();
    let after = element
        .next_siblings()
        .filter_map(ElementRef::wrap)
        .find(|sibling| !context::skips_text(sibling.value().name()))
        .map(|sibling| crate::parse::html::collapse_ws(&context::visible_text(&sibling)))
        .unwrap_or_default();
    (
        head_chars(&before, max_chars),
        head_chars(&after, max_chars),
    )
}

fn direct_text(element: &ElementRef<'_>) -> String {
    element
        .children()
        .filter_map(|child| match child.value() {
            Node::Text(text) => Some(text.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn stable_id(element: &ElementRef<'_>) -> Option<String> {
    element.attr("id").and_then(stable_token)
}

fn stable_classes(element: &ElementRef<'_>) -> Vec<String> {
    let mut tokens = BTreeSet::new();
    if let Some(classes) = element.attr("class") {
        for token in classes.split_whitespace() {
            if let Some(token) = stable_token(token) {
                tokens.insert(token);
            }
        }
    }
    tokens.into_iter().collect()
}

fn stable_token(raw: &str) -> Option<String> {
    let mut token = raw.trim().to_ascii_lowercase();
    if let Some((prefix, suffix)) = token.rsplit_once("__")
        && prefix.len() >= 3
        && suffix.len() >= 4
        && suffix.chars().any(|ch| ch.is_ascii_digit())
    {
        token = prefix.to_owned();
    }
    let length = token.chars().count();
    if !(3..=40).contains(&length) {
        return None;
    }
    let letters = token.chars().filter(|ch| ch.is_ascii_alphabetic()).count();
    if letters < 3 || has_digit_run(&token, 4) {
        return None;
    }
    if looks_hashed(&token) {
        return None;
    }
    Some(token)
}

fn looks_hashed(token: &str) -> bool {
    let suffix = token.rsplit_once('-').map_or(token, |(_, suffix)| suffix);
    if suffix.len() >= 6
        && suffix.chars().all(|ch| ch.is_ascii_hexdigit())
        && suffix.chars().any(|ch| ch.is_ascii_digit())
        && suffix.chars().any(|ch| ch.is_ascii_alphabetic())
    {
        return true;
    }
    token.len() >= 8
        && token.chars().all(|ch| ch.is_ascii_hexdigit())
        && token.chars().any(|ch| ch.is_ascii_digit())
        && token.chars().any(|ch| ch.is_ascii_alphabetic())
}

fn has_digit_run(token: &str, min: usize) -> bool {
    let mut run = 0usize;
    for ch in token.chars() {
        if ch.is_ascii_digit() {
            run += 1;
            if run >= min {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

fn normalize_for_match(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                cleaned.push(lower);
            }
        } else {
            cleaned.push(' ');
        }
    }
    crate::parse::html::collapse_ws(&cleaned)
}

fn dice(left: &str, right: &str) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    if left == right {
        return 1.0;
    }
    let left_chars: Vec<char> = left.chars().collect();
    let right_chars: Vec<char> = right.chars().collect();
    if left_chars.len() == 1 || right_chars.len() == 1 {
        return if left_chars == right_chars { 1.0 } else { 0.0 };
    }
    let mut left_bigrams: Vec<u64> = left_chars
        .windows(2)
        .map(|pair| bigram_key(pair[0], pair[1]))
        .collect();
    let mut right_bigrams: Vec<u64> = right_chars
        .windows(2)
        .map(|pair| bigram_key(pair[0], pair[1]))
        .collect();
    left_bigrams.sort_unstable();
    right_bigrams.sort_unstable();
    let (mut left_index, mut right_index, mut overlap) = (0usize, 0usize, 0usize);
    while left_index < left_bigrams.len() && right_index < right_bigrams.len() {
        match left_bigrams[left_index].cmp(&right_bigrams[right_index]) {
            std::cmp::Ordering::Less => left_index += 1,
            std::cmp::Ordering::Greater => right_index += 1,
            std::cmp::Ordering::Equal => {
                overlap += 1;
                left_index += 1;
                right_index += 1;
            }
        }
    }
    (2 * overlap) as f32 / (left_bigrams.len() + right_bigrams.len()) as f32
}

fn bigram_key(left: char, right: char) -> u64 {
    ((left as u64) << 32) | (right as u64)
}

fn path_similarity(fingerprint: &ElementFingerprint, path: &ElementPath) -> f32 {
    let candidate_ancestors: Vec<String> = path
        .ancestors
        .iter()
        .map(|segment| segment.tag.clone())
        .collect();
    let lcs = lcs_ratio(&fingerprint.ancestor_tags, &candidate_ancestors);
    let position = if fingerprint.depth == path.depth {
        let max = fingerprint.sibling_index.max(path.sibling_index);
        if max == 0 {
            1.0
        } else {
            1.0 - fingerprint.sibling_index.abs_diff(path.sibling_index) as f32 / (max + 1) as f32
        }
    } else {
        0.0
    };
    0.75 * lcs + 0.25 * position
}

fn lcs_ratio(left: &[String], right: &[String]) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let mut row = vec![0usize; right.len() + 1];
    for left_item in left {
        let mut previous = 0usize;
        for (index, right_item) in right.iter().enumerate() {
            let current = row[index + 1];
            if left_item == right_item {
                row[index + 1] = previous + 1;
            } else {
                row[index + 1] = row[index + 1].max(row[index]);
            }
            previous = current;
        }
    }
    row[right.len()] as f32 / left.len().max(right.len()) as f32
}
