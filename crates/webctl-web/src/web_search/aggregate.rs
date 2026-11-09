//! 多引擎结果聚合（0.2.2 M3，议题 #202）：URL 归一化、去重、RRF 融合。
//!
//! 设计取舍（写进 ROADMAP 的“诚实边界”）：
//! - 不同引擎的 rank **不可直接比较**，所以用 RRF（Reciprocal Rank Fusion，k=60）融合；
//! - 去重以**归一化 URL** 为准（去 utm/fbclid 等跟踪参数、fragment、尾斜杠、默认端口）；
//! - 同一 URL 被多个引擎命中时，`engines` 保留全部证据，`engine` 取 rank 最好的那个；
//! - 不做“跨引擎条数平均”之类的伪精度：RRF 只是排序手段，不宣称相关性分数。

use std::collections::HashMap;

use url::Url;

use super::{SearchEngine, SearchHit};

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

/// 把多引擎结果融合成一份列表。
///
/// - `inputs`：`(引擎, 该引擎的结果)`，顺序即优先级无关（RRF 只用 rank）；
/// - `max_results`：最终条数上限（去重后）。
pub fn merge(inputs: &[(SearchEngine, Vec<SearchHit>)], max_results: usize) -> Merged {
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
    for (url, _) in ordered.into_iter().take(max_results) {
        let Some((_, best_engine, mut hit)) = best.remove(&url) else {
            continue;
        };
        hit.engines = engines.remove(&url).unwrap_or_default();
        hit.engine = best_engine;
        hit.rank = merged.results.len() as u32 + 1;
        hit.is_ad |= ad_flags.remove(&url).unwrap_or(false);
        merged.results.push(hit);
    }
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
        let merged = merge(&inputs, 10);
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
        let merged = merge(&inputs, 10);
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
        let merged = merge(&inputs, 2);
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
        let merged = merge(&inputs, 10);
        assert_eq!(merged.results[0].engine, "bing");
        assert_eq!(merged.results[0].engines.len(), 2);
    }

    #[test]
    fn drops_non_http_hits() {
        let inputs = vec![(
            SearchEngine::Bing,
            vec![hit(SearchEngine::Bing, 1, "javascript:alert(1)")],
        )];
        assert!(merge(&inputs, 10).results.is_empty());
    }
}
