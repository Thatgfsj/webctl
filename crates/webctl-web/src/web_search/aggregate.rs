//! 多引擎结果聚合（0.2.2 M3，议题 #202）：URL 归一化、去重、RRF 融合。
//!
//! 设计取舍（写进 ROADMAP 的“诚实边界”）：
//! - 不同引擎的 rank **不可直接比较**，所以用 RRF（Reciprocal Rank Fusion，k=60）融合；
//! - 去重以**归一化 URL** 为准（去 utm/fbclid 等跟踪参数、fragment、尾斜杠、默认端口）；
//! - 同一 URL 被多个引擎命中时，`engines` 保留全部证据，`engine` 取 rank 最好的那个；
//! - 不做“跨引擎条数平均”之类的伪精度：RRF 只是排序手段，不宣称相关性分数。

use std::collections::HashMap;

use url::Url;

use super::{DEFAULT_MAX_RESULTS, DEFAULT_SNIPPET_MAX_CHARS, SearchEngine, SearchHit};

/// RRF 常数（论文默认 60）。
pub const RRF_K: f64 = 60.0;

/// 需要剥离的跟踪参数（精确匹配）。
const TRACKING_PARAMS: &[&str] = &[
    "fbclid", "gclid", "msclkid", "yclid", "spm", "ref", "ref_src", "source",
];

/// URL 归一化：只保留 http(s)；去掉 fragment、跟踪参数、尾斜杠、默认端口。
pub fn normalize_url(raw: &str) -> Option<String> {
    let mut url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.set_fragment(None);
    if let Some(host) = url.host_str().map(str::to_ascii_lowercase) {
        let _ = url.set_host(Some(&host));
    }
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| {
            let key = key.as_ref();
            !TRACKING_PARAMS.contains(&key) && !key.starts_with("utm_")
        })
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        let mut pairs = url.query_pairs_mut();
        pairs.clear();
        for (key, value) in &kept {
            pairs.append_pair(key, value);
        }
    }
    let path = url.path().to_owned();
    if path.len() > 1 && path.ends_with('/') {
        url.set_path(&path[..path.len() - 1]);
    }
    Some(url.to_string())
}

/// 融合后的结果与逐条证据。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Merged {
    /// 按 RRF 分数降序的结果（rank 从 1 重新编号）。
    pub results: Vec<SearchHit>,
    /// 去重前各引擎的原始条数（供状态展示）。
    pub raw_counts: HashMap<String, usize>,
}

/// 聚合选项（0.2.3 M3，议题 #223）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeOptions {
    /// 最终条数上限（在**同域折叠之后**截断）。
    pub max_results: usize,
    /// 是否展开同域结果；默认 false（同域折叠为一条主结果 + `same_domain_count`）。
    pub expand_domains: bool,
    /// snippet 最大 Unicode 字符数；0 表示不截断。
    pub snippet_max_chars: usize,
}

impl Default for MergeOptions {
    fn default() -> Self {
        Self {
            max_results: DEFAULT_MAX_RESULTS,
            expand_domains: false,
            snippet_max_chars: DEFAULT_SNIPPET_MAX_CHARS,
        }
    }
}

/// 按 Unicode 字符截断 snippet；limit==0 或不足 limit 时原样返回。
///
/// 返回 `(截断后文本, 是否截断)`。按 `char` 迭代，绝不按字节切，
/// 因此中文/emoji 不会被切成半个码点。
pub fn truncate_snippet(snippet: &str, limit: usize) -> (String, bool) {
    if limit == 0 {
        return (snippet.to_owned(), false);
    }
    let mut chars = snippet.chars();
    let mut kept = String::new();
    for _ in 0..limit {
        match chars.next() {
            Some(ch) => kept.push(ch),
            None => return (snippet.to_owned(), false),
        }
    }
    if chars.next().is_none() {
        // 恰好等于 limit：不算截断，原样返回。
        return (snippet.to_owned(), false);
    }
    (kept, true)
}

/// 同域折叠用的域键：URL host 小写并去掉前导 `www.`；无 host 时退回自身 URL。
///
/// 已知限制：不查 PSL，`a.example.co.uk` 与 `b.example.co.uk` 的 host 不同，
/// 会被当作两个域（不做 eTLD+1 归并）。
fn domain_key(url: &str) -> String {
    match Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
    {
        Some(host) => {
            let lower = host.to_ascii_lowercase();
            lower
                .strip_prefix("www.")
                .unwrap_or(lower.as_str())
                .to_owned()
        }
        None => url.to_owned(),
    }
}

/// 折叠同域结果：保留该域排名最靠前的一条，`same_domain_count=原条数`；顺序按首个出现位置。
pub fn collapse_by_domain(hits: Vec<SearchHit>) -> Vec<SearchHit> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<SearchHit>> = HashMap::new();
    for hit in hits {
        let key = domain_key(&hit.url);
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(hit);
    }
    let mut collapsed = Vec::with_capacity(order.len());
    for key in order {
        let Some(mut group) = groups.remove(&key) else {
            continue;
        };
        let count = group.len();
        let mut primary = group.remove(0);
        if count >= 2 {
            primary.same_domain_count = Some(count);
        }
        collapsed.push(primary);
    }
    collapsed
}

/// 把多引擎结果融合成一份列表。
///
/// - `inputs`：`(引擎, 该引擎的结果)`，顺序即优先级无关（RRF 只用 rank）；
/// - `options`：条数上限、是否展开同域、snippet 字符上限。
///
/// 顺序固定为：RRF 排名 → snippet 截断（标记 `snippet_truncated`）→ 同域折叠
/// （除非 `expand_domains`）→ 截到 `max_results`。折叠必须在 `max_results` 之前，
/// 否则同域条目会挤掉本应保留的其它域结果。
pub fn merge(inputs: &[(SearchEngine, Vec<SearchHit>)], options: MergeOptions) -> Merged {
    let mut merged = Merged::default();
    let mut scores: HashMap<String, f64> = HashMap::new();
    let mut best: HashMap<String, (usize, String, SearchHit)> = HashMap::new();
    let mut engines: HashMap<String, Vec<String>> = HashMap::new();
    // 低置信广告标记跨引擎取并集：任一引擎判为广告，融合结果就保留标记。
    let mut ad_flags: HashMap<String, bool> = HashMap::new();
    for (engine, hits) in inputs {
        let name = engine.as_str().to_owned();
        merged.raw_counts.insert(name.clone(), hits.len());
        for (index, hit) in hits.iter().enumerate() {
            let Some(url) = normalize_url(&hit.url) else {
                continue;
            };
            let rank = index + 1;
            *scores.entry(url.clone()).or_insert(0.0) += 1.0 / (RRF_K + rank as f64);
            let entry = engines.entry(url.clone()).or_default();
            if !entry.contains(&name) {
                entry.push(name.clone());
            }
            *ad_flags.entry(url.clone()).or_insert(false) |= hit.is_ad;
            match best.get(&url) {
                Some((best_rank, _, _)) if *best_rank <= rank => {}
                _ => {
                    let mut record = hit.clone();
                    record.url = url.clone();
                    best.insert(url.clone(), (rank, name.clone(), record));
                }
            }
        }
    }
    let mut ordered: Vec<(String, f64)> = scores.into_iter().collect();
    ordered.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut results: Vec<SearchHit> = Vec::with_capacity(ordered.len());
    for (url, _) in ordered {
        let Some((_, best_engine, mut hit)) = best.remove(&url) else {
            continue;
        };
        hit.engines = engines.remove(&url).unwrap_or_default();
        hit.engine = best_engine;
        hit.rank = results.len() as u32 + 1;
        hit.is_ad |= ad_flags.remove(&url).unwrap_or(false);
        let (snippet, truncated) = truncate_snippet(&hit.snippet, options.snippet_max_chars);
        hit.snippet = snippet;
        hit.snippet_truncated = truncated;
        results.push(hit);
    }
    // 折叠必须在 max_results 截断之前：否则同域条目会挤掉其它域的结果。
    if !options.expand_domains {
        results = collapse_by_domain(results);
    }
    results.truncate(options.max_results);
    for (index, hit) in results.iter_mut().enumerate() {
        hit.rank = index as u32 + 1;
    }
    merged.results = results;
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(engine: SearchEngine, rank: u32, url: &str) -> SearchHit {
        SearchHit {
            rank,
            engine: engine.as_str().to_owned(),
            title: format!("{engine:?} #{rank}"),
            url: url.to_owned(),
            display_url: String::new(),
            snippet: String::new(),
            engines: vec![engine.as_str().to_owned()],
            is_ad: false,
            same_domain_count: None,
            snippet_truncated: false,
        }
    }

    fn options(max_results: usize) -> MergeOptions {
        MergeOptions {
            max_results,
            ..MergeOptions::default()
        }
    }

    #[test]
    fn normalizes_tracking_params_fragment_and_trailing_slash() {
        let a = normalize_url("https://Example.COM/a/?utm_source=x&id=1#frag").expect("url");
        assert_eq!(a, "https://example.com/a?id=1");
        assert_eq!(normalize_url("ftp://example.com/x"), None);
        assert_eq!(normalize_url("not a url"), None);
    }

    #[test]
    fn dedupes_same_url_and_keeps_engine_evidence() {
        let inputs = vec![
            (
                SearchEngine::Bing,
                vec![hit(SearchEngine::Bing, 1, "https://a.test/x?utm_source=1")],
            ),
            (
                SearchEngine::Baidu,
                vec![hit(SearchEngine::Baidu, 1, "https://a.test/x")],
            ),
        ];
        let merged = merge(&inputs, options(10));
        assert_eq!(merged.results.len(), 1);
        assert_eq!(merged.results[0].engines.len(), 2);
    }

    #[test]
    fn rrf_prefers_multi_engine_hits() {
        let inputs = vec![
            (
                SearchEngine::Bing,
                vec![
                    hit(SearchEngine::Bing, 1, "https://one.test/"),
                    hit(SearchEngine::Bing, 2, "https://both.test/"),
                ],
            ),
            (
                SearchEngine::Baidu,
                vec![hit(SearchEngine::Baidu, 1, "https://both.test/")],
            ),
        ];
        let merged = merge(&inputs, options(10));
        assert_eq!(merged.results[0].url, "https://both.test/");
        assert_eq!(merged.results[0].engines.len(), 2);
    }

    #[test]
    fn respects_max_results_and_reranks() {
        let inputs = vec![(
            SearchEngine::Bing,
            vec![
                hit(SearchEngine::Bing, 1, "https://a.test/"),
                hit(SearchEngine::Bing, 2, "https://b.test/"),
                hit(SearchEngine::Bing, 3, "https://c.test/"),
            ],
        )];
        let merged = merge(&inputs, options(2));
        assert_eq!(merged.results.len(), 2);
        assert_eq!(merged.results[0].rank, 1);
        assert_eq!(merged.results[1].rank, 2);
    }

    #[test]
    fn keeps_non_default_ports() {
        assert_eq!(
            normalize_url("https://example.com:8443/a").as_deref(),
            Some("https://example.com:8443/a")
        );
        // 默认端口由 url crate 规范化，归一化后不出现 :443。
        let default_port = normalize_url("https://example.com:443/a").expect("url");
        assert_eq!(default_port, "https://example.com/a");
    }

    #[test]
    fn engine_label_follows_best_rank() {
        let inputs = vec![
            (
                SearchEngine::Bing,
                vec![hit(SearchEngine::Bing, 1, "https://a.test/")],
            ),
            (
                SearchEngine::Baidu,
                vec![hit(SearchEngine::Baidu, 3, "https://a.test/")],
            ),
        ];
        let merged = merge(&inputs, options(10));
        assert_eq!(merged.results[0].engine, "bing");
        assert_eq!(merged.results[0].engines.len(), 2);
    }

    #[test]
    fn drops_non_http_hits() {
        let inputs = vec![(
            SearchEngine::Bing,
            vec![hit(SearchEngine::Bing, 1, "javascript:alert(1)")],
        )];
        assert!(merge(&inputs, options(10)).results.is_empty());
    }

    #[test]
    fn truncate_snippet_counts_unicode_chars_not_bytes() {
        let chinese = "中".repeat(301);
        let (text, truncated) = truncate_snippet(&chinese, 300);
        assert!(truncated);
        assert_eq!(text.chars().count(), 300);
        assert_eq!(text, "中".repeat(300));
        assert!(!text.contains('\u{FFFD}'), "不得出现 UTF-8 替换字符");

        // 恰好 300 字符：不截断，原样返回。
        let exact = "中".repeat(300);
        let (text, truncated) = truncate_snippet(&exact, 300);
        assert!(!truncated);
        assert_eq!(text, exact);

        // limit=0 表示不截断。
        let (text, truncated) = truncate_snippet(&chinese, 0);
        assert!(!truncated);
        assert_eq!(text, chinese);

        // emoji 是 4 字节单码点，按 char 截断不会切半。
        let emoji = "😀😀😀😀";
        let (text, truncated) = truncate_snippet(emoji, 2);
        assert!(truncated);
        assert_eq!(text, "😀😀");
        assert_eq!(text.chars().count(), 2);
        assert!(!text.contains('\u{FFFD}'), "emoji 不得被切半");
    }

    #[test]
    fn truncate_snippet_keeps_empty_and_short_values() {
        assert_eq!(truncate_snippet("", 300), (String::new(), false));
        assert_eq!(truncate_snippet("short", 300), ("short".to_owned(), false));
    }

    #[test]
    fn collapse_keeps_first_hit_per_domain_and_counts() {
        let hits = vec![
            hit(SearchEngine::Bing, 1, "https://www.example.com/a"),
            hit(SearchEngine::Bing, 2, "https://example.com/b"),
            hit(SearchEngine::Bing, 3, "https://other.test/c"),
        ];
        let collapsed = collapse_by_domain(hits);
        assert_eq!(collapsed.len(), 2);
        // 主结果排名保持（取首个出现位置的那条）。
        assert_eq!(collapsed[0].url, "https://www.example.com/a");
        assert_eq!(collapsed[0].rank, 1);
        assert_eq!(collapsed[0].same_domain_count, Some(2));
        assert_eq!(collapsed[1].url, "https://other.test/c");
        assert_eq!(collapsed[1].same_domain_count, None);
    }

    #[test]
    fn collapse_counts_three_same_domain_hits() {
        let hits = vec![
            hit(SearchEngine::Bing, 1, "https://example.com/1"),
            hit(SearchEngine::Bing, 2, "https://example.com/2"),
            hit(SearchEngine::Bing, 3, "https://example.com/3"),
        ];
        let collapsed = collapse_by_domain(hits);
        assert_eq!(collapsed.len(), 1);
        assert_eq!(collapsed[0].same_domain_count, Some(3));
    }

    #[test]
    fn collapse_is_case_insensitive_and_strips_www_only() {
        let hits = vec![
            hit(SearchEngine::Bing, 1, "https://WWW.Example.COM/a"),
            hit(SearchEngine::Bing, 2, "https://example.com/b"),
            // 已知限制：不做 PSL，子域各算一个域。
            hit(SearchEngine::Bing, 3, "https://a.example.co.uk/c"),
            hit(SearchEngine::Bing, 4, "https://b.example.co.uk/d"),
        ];
        let collapsed = collapse_by_domain(hits);
        assert_eq!(collapsed.len(), 3);
        assert_eq!(collapsed[0].same_domain_count, Some(2));
        assert_eq!(collapsed[1].same_domain_count, None);
        assert_eq!(collapsed[2].same_domain_count, None);
    }

    #[test]
    fn collapse_does_not_panic_without_host() {
        // 无 host 时按自身 URL 作键：相同的坏 URL 折叠，不同的不折叠。
        let hits = vec![
            hit(SearchEngine::Bing, 1, "not a url"),
            hit(SearchEngine::Bing, 2, "not a url"),
            hit(SearchEngine::Bing, 3, "also not a url"),
        ];
        let collapsed = collapse_by_domain(hits);
        assert_eq!(collapsed.len(), 2);
        assert_eq!(collapsed[0].url, "not a url");
        assert_eq!(collapsed[0].same_domain_count, Some(2));
    }

    #[test]
    fn merge_collapses_same_domain_by_default() {
        let inputs = vec![(
            SearchEngine::Bing,
            vec![
                hit(SearchEngine::Bing, 1, "https://example.com/a"),
                hit(SearchEngine::Bing, 2, "https://example.com/b"),
                hit(SearchEngine::Bing, 3, "https://other.test/c"),
            ],
        )];
        let merged = merge(&inputs, options(10));
        assert_eq!(merged.results.len(), 2);
        assert_eq!(merged.results[0].url, "https://example.com/a");
        assert_eq!(merged.results[0].same_domain_count, Some(2));
        assert_eq!(merged.results[1].url, "https://other.test/c");
        assert_eq!(merged.results[1].same_domain_count, None);
    }

    #[test]
    fn merge_keeps_same_domain_when_expanded() {
        let inputs = vec![(
            SearchEngine::Bing,
            vec![
                hit(SearchEngine::Bing, 1, "https://example.com/a"),
                hit(SearchEngine::Bing, 2, "https://example.com/b"),
            ],
        )];
        let merged = merge(
            &inputs,
            MergeOptions {
                max_results: 10,
                expand_domains: true,
                snippet_max_chars: 300,
            },
        );
        assert_eq!(merged.results.len(), 2);
        assert!(
            merged
                .results
                .iter()
                .all(|hit| hit.same_domain_count.is_none())
        );
    }

    #[test]
    fn merge_truncates_snippets_and_marks_them() {
        let mut long = hit(SearchEngine::Bing, 1, "https://example.com/a");
        long.snippet = "中".repeat(301);
        let merged = merge(
            &[(SearchEngine::Bing, vec![long.clone()])],
            MergeOptions {
                max_results: 10,
                expand_domains: true,
                snippet_max_chars: 50,
            },
        );
        assert_eq!(merged.results[0].snippet.chars().count(), 50);
        assert!(merged.results[0].snippet_truncated);

        let merged = merge(
            &[(SearchEngine::Bing, vec![long])],
            MergeOptions {
                max_results: 10,
                expand_domains: true,
                snippet_max_chars: 0,
            },
        );
        assert_eq!(merged.results[0].snippet.chars().count(), 301);
        assert!(!merged.results[0].snippet_truncated);
    }

    #[test]
    fn merge_collapses_before_truncating_to_max_results() {
        // 两个域各 2 条；若先按 max_results 截断再折叠，只会剩 1 条。
        let inputs = vec![(
            SearchEngine::Bing,
            vec![
                hit(SearchEngine::Bing, 1, "https://a.test/1"),
                hit(SearchEngine::Bing, 2, "https://a.test/2"),
                hit(SearchEngine::Bing, 3, "https://b.test/1"),
                hit(SearchEngine::Bing, 4, "https://b.test/2"),
            ],
        )];
        let merged = merge(&inputs, options(2));
        assert_eq!(merged.results.len(), 2, "折叠后两个域都应保留");
        assert_eq!(merged.results[0].url, "https://a.test/1");
        assert_eq!(merged.results[0].same_domain_count, Some(2));
        assert_eq!(merged.results[1].url, "https://b.test/1");
        assert_eq!(merged.results[1].same_domain_count, Some(2));
        assert_eq!(merged.results[0].rank, 1);
        assert_eq!(merged.results[1].rank, 2);
    }
}
