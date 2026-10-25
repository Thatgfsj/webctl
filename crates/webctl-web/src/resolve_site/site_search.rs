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

/// L2 候选的置信分：低于 P856（90）与内置表（95），高于 TLD 直推。
pub const SCORE: u8 = 65;

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
    let mut candidates: Vec<Candidate> = Vec::new();
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
            let candidate_url = normalize_url(&target);
            if candidates.iter().any(|known| known.url == candidate_url) {
                continue;
            }
            candidates.push(Candidate {
                url: candidate_url,
                score: SCORE,
                evidence: format!(
                    "{} 站内搜索第 {} 条：{}",
                    template.site,
                    hits + 1,
                    link.text.trim()
                ),
                channel: "site-search",
            });
            hits += 1;
        }
        reports.push(SiteSearchReport {
            site: template.site.to_owned(),
            status: if hits == 0 {
                SiteSearchStatus::Empty
            } else {
                SiteSearchStatus::Hit
            },
            candidates: hits,
            note: if hits == 0 {
                "结果页没有与名称匹配的外部链接".to_owned()
            } else {
                format!("{hits} 条匹配的外部链接")
            },
        });
    }
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

/// 链接文本或 URL 里是否包含名称（大小写不敏感、忽略空白）。
fn matches_name(text: &str, href: &str, normalized: &str) -> bool {
    if normalized.is_empty() {
        return false;
    }
    let haystack = format!(
        "{} {}",
        text.to_ascii_lowercase(),
        href.to_ascii_lowercase()
    );
    let compact: String = haystack.split_whitespace().collect();
    let needle: String = normalized.split_whitespace().collect();
    compact.contains(&needle)
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
