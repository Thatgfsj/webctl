//! L2 通道（ADR-0006）：**站点内建搜索**（模板化搜索入口）。
//!
//! 做法：把站点名丢进若干"已知站点自带的搜索页"（模板 = URL + {query} 占位），
//! 从结果页里抽**外部链接**、只保留与名称匹配的那些，作为候选。
//!
//! 为什么只保留"外部链接 + 名称匹配"：
//! - 结果页内部链接（同一站点自己的页面）不是"站点入口"，必须过滤；
//! - 名称不匹配的结果是无关噪声，宁可空也不猜。
//!
//! 诚实边界：
//! - **先过 politeness**：robots 拦下**不请求**，逐站点如实回报（note 里保留 robots 原因）；
//! - 每次调用最多探测 [`MAX_SITES`] 个模板（每个一次请求，给工具预算留余量）；
//! - 候选分数 **65**（低于 Wikidata P856 的 90，高于 TLD 直推）；
//! - 失败/空结果**逐站点记录**，不聚合成一句"没找到"；
//! - 现实里很多站点的搜索页是 JS 渲染或被 robots 禁止 —— 这种情况如实标 Empty/Unavailable，
//!   本通道**不承诺**命中（真实命中率未验证，见 PLAN §9）。

use url::Url;

use super::Candidate;
use crate::extract::{self, ExtractOptions, LinkFilter};
use crate::politeness::PoliteFetcher;

// 编译期不变量：两种 L2 分值都必须低于工具的可靠阈值 60（L2 永远不能把 confidence 变成 high）。
const _: () = assert!(SCORE_SINGLE < 60);
const _: () = assert!(SCORE_CORROBORATED < 60);
const _: () = assert!(SCORE_CORROBORATED > SCORE_SINGLE);

/// 单站点命中的置信分：**低于可靠阈值（工具的 60）**，也低于内置表 95 / P856 90，
/// 与 .com 直推同档（55），低于"名称本身即域名"的直推（70）——
/// L2 只是"名称匹配的外部链接"，不足以算可靠定位。
pub const SCORE_SINGLE: u8 = 55;

/// 跨站点互证（≥2 个模板都命中）时的分值：仍**低于**可靠阈值 60，
/// 只是排序更靠前 —— 互证提高可信度，但不改变"L2 不算可靠定位"的结论。
pub const SCORE_CORROBORATED: u8 = 58;

/// 每次调用最多探测几个站点模板。
pub const MAX_SITES: usize = 2;

/// 一个站点的搜索入口模板（`{query}` 占位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchTemplate {
    /// 站点标识（写进证据）。
    pub site: &'static str,
    /// 搜索 URL 模板。
    pub url: &'static str,
}

/// 内置模板表（24 条；顺序 = 探测顺序，优先"结果页更可能含外部链接"的聚合/百科站）。
pub const TEMPLATES: &[SearchTemplate] = &[
    SearchTemplate {
        site: "hacker-news",
        url: "https://hn.algolia.com/?q={query}",
    },
    SearchTemplate {
        site: "reddit",
        url: "https://www.reddit.com/search/?q={query}",
    },
    SearchTemplate {
        site: "wikipedia-zh",
        url: "https://zh.wikipedia.org/w/index.php?search={query}",
    },
    SearchTemplate {
        site: "wikipedia-en",
        url: "https://en.wikipedia.org/w/index.php?search={query}",
    },
    SearchTemplate {
        site: "github",
        url: "https://github.com/search?q={query}&type=repositories",
    },
    SearchTemplate {
        site: "gitlab",
        url: "https://gitlab.com/search?search={query}",
    },
    SearchTemplate {
        site: "stackoverflow",
        url: "https://stackoverflow.com/search?q={query}",
    },
    SearchTemplate {
        site: "npm",
        url: "https://www.npmjs.com/search?q={query}",
    },
    SearchTemplate {
        site: "crates-io",
        url: "https://crates.io/search?q={query}",
    },
    SearchTemplate {
        site: "pypi",
        url: "https://pypi.org/search/?q={query}",
    },
    SearchTemplate {
        site: "dockerhub",
        url: "https://hub.docker.com/search?q={query}",
    },
    SearchTemplate {
        site: "huggingface",
        url: "https://huggingface.co/search/full-text?q={query}",
    },
    SearchTemplate {
        site: "mdn",
        url: "https://developer.mozilla.org/zh-CN/search?q={query}",
    },
    SearchTemplate {
        site: "zhihu",
        url: "https://www.zhihu.com/search?q={query}",
    },
    SearchTemplate {
        site: "douban",
        url: "https://www.douban.com/search?q={query}",
    },
    SearchTemplate {
        site: "bilibili",
        url: "https://search.bilibili.com/all?keyword={query}",
    },
    SearchTemplate {
        site: "weibo",
        url: "https://s.weibo.com/weibo?q={query}",
    },
    SearchTemplate {
        site: "youtube",
        url: "https://www.youtube.com/results?search_query={query}",
    },
    SearchTemplate {
        site: "imdb",
        url: "https://www.imdb.com/find/?q={query}",
    },
    SearchTemplate {
        site: "medium",
        url: "https://medium.com/search?q={query}",
    },
    SearchTemplate {
        site: "devto",
        url: "https://dev.to/search?q={query}",
    },
    SearchTemplate {
        site: "producthunt",
        url: "https://www.producthunt.com/search?q={query}",
    },
    SearchTemplate {
        site: "arxiv",
        url: "https://arxiv.org/search/?query={query}",
    },
];

/// L2 配置（模板表可注入；测试因此指向本地 fixture）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteSearchConfig {
    /// 模板表（生产用 [`TEMPLATES`]）。
    pub templates: Vec<SearchTemplate>,
    /// 每次最多探测几个模板。
    pub max_sites: usize,
    /// 每个结果页最多取几条链接。
    pub max_results: usize,
}

impl Default for SiteSearchConfig {
    fn default() -> Self {
        Self {
            templates: TEMPLATES.to_vec(),
            max_sites: MAX_SITES,
            max_results: 10,
        }
    }
}

/// 单站点状态（三态必须分开）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteSearchStatus {
    /// 页面可读且有命中。
    Hit,
    /// 页面可读但没有可用结果。
    Empty,
    /// 没拿到页面（robots 禁止 / HTTP 错误 / 网络失败）。
    Unavailable,
}

/// 单个模板的探测结果（逐站点如实记录）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SiteSearchReport {
    /// 站点标识。
    pub site: String,
    /// 结果状态。
    pub status: SiteSearchStatus,
    /// 命中的候选数。
    pub candidates: usize,
    /// 可读说明（被 robots 拦下时保留 robots 原因）。
    pub note: String,
}

/// L2 查询结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteSearchOutcome {
    /// 候选（按探测顺序、结果页顺序）。
    pub candidates: Vec<Candidate>,
    /// 逐站点报告。
    pub reports: Vec<SiteSearchReport>,
}

/// 依次探测模板，收集候选。
pub async fn search(
    name: &str,
    config: &SiteSearchConfig,
    fetcher: &PoliteFetcher,
) -> SiteSearchOutcome {
    let normalized = normalize_name(name);
    let mut collected: Vec<(usize, Candidate)> = Vec::new();
    let mut hit_templates: Vec<usize> = Vec::new();
    let mut reports: Vec<SiteSearchReport> = Vec::new();
    for template in config.templates.iter().take(config.max_sites.max(1)) {
        let url = template
            .url
            .replace("{query}", &percent_encode(name.trim()));
        let envelope = fetcher.fetch(&url).await;
        let Some(fetched) = envelope.data else {
            reports.push(SiteSearchReport {
                site: template.site.to_owned(),
                status: SiteSearchStatus::Unavailable,
                candidates: 0,
                note: format!("{}：{}", envelope.code.as_str(), envelope.error),
            });
            continue;
        };
        let Some(html) = fetched.fetched.content.text else {
            reports.push(SiteSearchReport {
                site: template.site.to_owned(),
                status: SiteSearchStatus::Empty,
                candidates: 0,
                note: "响应没有可解析的文本正文".to_owned(),
            });
            continue;
        };
        let base_url = fetched.fetched.final_url.clone();
        let site_host = Url::parse(&base_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default();
        let options = ExtractOptions {
            max_results: config.max_results.max(1),
            ..ExtractOptions::default()
        };
        let links = extract::links(&html, Some(&base_url), &LinkFilter::default(), &options);
        let Some(filtered) = links.data else {
            reports.push(SiteSearchReport {
                site: template.site.to_owned(),
                status: SiteSearchStatus::Empty,
                candidates: 0,
                note: format!("结果页解析失败：{}", links.error),
            });
            continue;
        };
        let mut hits = 0usize;
        let mut matched = 0usize;
        let template_index = reports.len();
        for link in filtered.links {
            if !matches_name(&link.text, &link.href, &normalized) {
                continue;
            }
            let Ok(target) = Url::parse(&link.href) else {
                continue;
            };
            if !matches!(target.scheme(), "http" | "https") {
                continue;
            }
            // 站点自身链接不是"入口"，必须过滤（否则等于把自己当答案）。
            if target.host_str().is_some_and(|host| host == site_host) {
                continue;
            }
            matched += 1;
            let candidate_url = normalize_url(&target);
            // 去重只影响"要不要新增候选"，不影响"这个模板命中过"（互证按站点算）。
            if collected
                .iter()
                .any(|(_, known)| known.url == candidate_url)
            {
                continue;
            }
            collected.push((
                template_index,
                Candidate {
                    url: candidate_url,
                    score: SCORE_SINGLE,
                    evidence: format!(
                        "{} 站内搜索第 {} 条：{}",
                        template.site,
                        matched,
                        link.text.trim()
                    ),
                    channel: "site-search",
                },
            ));
            hits += 1;
        }
        if matched > 0 {
            // 记录"哪个模板命中"——互证是**站点维度**的证据，不能被 URL 去重抹掉。
            hit_templates.push(template_index);
        }
        reports.push(SiteSearchReport {
            site: template.site.to_owned(),
            status: if matched == 0 {
                SiteSearchStatus::Empty
            } else {
                SiteSearchStatus::Hit
            },
            candidates: matched,
            note: if matched == 0 {
                "结果页没有与名称匹配的外部链接".to_owned()
            } else {
                format!("{matched} 条匹配的外部链接（其中 {hits} 条新增候选）")
            },
        });
    }
    // 互证：有多少个不同模板真的命中了（≥2 才把分数提到 SCORE_CORROBORATED）。
    hit_templates.sort_unstable();
    hit_templates.dedup();
    let corroborated = hit_templates.len() >= 2;
    let score = if corroborated {
        SCORE_CORROBORATED
    } else {
        SCORE_SINGLE
    };
    let candidates: Vec<Candidate> = collected
        .into_iter()
        .map(|(_, mut candidate)| {
            candidate.score = score;
            if corroborated {
                candidate
                    .evidence
                    .push_str("（本批站点搜索有 ≥2 个站点命中）");
            }
            candidate
        })
        .collect();
    SiteSearchOutcome {
        candidates,
        reports,
    }
}

/// 名称规范化（去空白、小写、去"官网"等后缀）。
fn normalize_name(name: &str) -> String {
    let mut text = name.trim().to_ascii_lowercase();
    for suffix in [" 官网", "官网", " official site", " website"] {
        if let Some(stripped) = text.strip_suffix(suffix) {
            text = stripped.trim().to_owned();
        }
    }
    text
}

/// 结果是否与该名称有关。
///
/// 只认两条**保守**判据（不再对原始 href 做子串匹配——短名 ai/app 会误配 detail/download）：
/// 1. 链接文本里名称按**词边界**出现（前后不是字母数字）；
/// 2. 目标 host 的某个 label（去掉 www 与连字符）与规范化名称完全相等。
fn matches_name(text: &str, href: &str, normalized: &str) -> bool {
    if normalized.is_empty() {
        return false;
    }
    let needle = normalized
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if let Some(position) = text.to_ascii_lowercase().find(&needle) {
        let before = text[..position].chars().next_back();
        let after = text[position + needle.len()..].chars().next();
        let boundary = |ch: Option<char>| ch.is_none_or(|c| !c.is_alphanumeric());
        if boundary(before) && boundary(after) {
            return true;
        }
    }
    let Some(host) = url::Url::parse(href)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
    else {
        return false;
    };
    let compact_name: String = normalized
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-')
        .collect();
    host.to_ascii_lowercase()
        .split('.')
        .filter(|label| *label != "www")
        .any(|label| label.replace('-', "") == compact_name)
}

/// 规范化候选 URL：https、去尾斜杠。
fn normalize_url(url: &Url) -> String {
    let mut text = url.to_string();
    if text.starts_with("http://") {
        text = text.replacen("http://", "https://", 1);
    }
    text.trim_end_matches('/').to_owned()
}

/// 查询串组件编码（空格 -> %20）。
fn percent_encode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_between_20_and_50() {
        assert!(
            (20..=50).contains(&TEMPLATES.len()),
            "模板数量应在 20–50：{}",
            TEMPLATES.len()
        );
    }

    #[test]
    fn templates_use_https_and_query_placeholder() {
        for template in TEMPLATES {
            assert!(
                template.url.starts_with("https://"),
                "模板必须 https：{}",
                template.url
            );
            assert!(
                template.url.contains("{query}"),
                "模板必须带 query 占位：{}",
                template.url
            );
        }
    }

    #[test]
    fn matches_name_ignores_case_and_spaces() {
        assert!(matches_name(
            "Hacker News",
            "https://news.ycombinator.com",
            "hacker news"
        ));
        assert!(matches_name("GitHub", "https://github.com/x", "github"));
        assert!(!matches_name("无关结果", "https://example.com", "某站"));
    }

    /// 短名不得在**单词内部**误配（否则 ai/app/news 这类名字会把 detail/download 当命中）。
    #[test]
    fn short_names_do_not_match_inside_words() {
        assert!(!matches_name("detail page", "https://example.com/x", "ai"));
        assert!(!matches_name("downloads", "https://example.com/x", "app"));
        // 注意：host label 等于名称是**设计上的命中**，所以这里用无关 host 验证"不误配"。
        assert!(!matches_name("unrelated", "https://example.com", "news"));
    }

    /// host label 完全相等也算命中（名称与域名同形时，文本里不一定出现）。
    #[test]
    fn host_label_equal_to_name_counts_as_match() {
        assert!(matches_name("某站首页", "https://github.com/x", "github"));
        assert!(matches_name("某站", "https://www.example.com/", "example"));
        assert!(!matches_name("某站", "https://notexample.com/", "example"));
    }

    #[test]
    fn normalize_name_strips_official_site_suffix() {
        assert_eq!(normalize_name("  Hacker News 官网 "), "hacker news");
        assert_eq!(normalize_name("OpenAI official site"), "openai");
    }

    #[test]
    fn percent_encode_escapes_cjk() {
        assert_eq!(percent_encode("知乎"), "%E7%9F%A5%E4%B9%8E");
    }
}
