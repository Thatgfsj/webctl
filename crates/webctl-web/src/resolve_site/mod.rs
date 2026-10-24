//! 站点定位（0.2 / ADR-0006）：名称 -> 候选 URL。
//!
//! - **L1 层**（本文件）：内置映射表 + 域名直推，**不联网** ✓；
//! - **L3 层**（子模块 wikidata）：免 key 公共只读 API（Wikidata P856 官方网站），联网且过 politeness；
//! - **L4 层**（子模块 sitemap）：候选域名 robots.txt 的 Sitemap 声明，**只报告声明**（不据此提升置信度）；
//! - L2 站点内建搜索、L5 SERP 兜底仍在后续切片。
//!
//! 诚实性约束：
//! - **绝不返回单一"看起来像"的答案**：永远给候选列表 + 置信分 + 证据 ✗；
//! - 不认识就返回**空**（不是猜）✗；
//! - 每个候选都带 `channel`，调用方能看出它来自"内置表"还是"直推"。

pub mod sitemap;
pub mod wikidata;

use serde::{Deserialize, Serialize};

/// 一个候选站点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// 规范化后的 URL（https + 无尾斜杠）。
    pub url: String,
    /// 置信分 0-100（内置表 95；常见 TLD 直推 40-60）。
    pub score: u8,
    /// 人类可读证据（例如「内置映射表：知乎 -> zhihu.com」）。
    pub evidence: String,
    /// 来源层：`builtin` / `direct`。
    pub channel: &'static str,
}

/// 内置映射表（常见站点名 -> 域名）。**只收录确定无疑的**；宁可缺，不可错。
pub const BUILTIN_SITES: &[(&str, &str)] = &[
    ("github", "github.com"),
    ("gitlab", "gitlab.com"),
    ("hacker news", "news.ycombinator.com"),
    ("hackernews", "news.ycombinator.com"),
    ("ycombinator", "news.ycombinator.com"),
    ("wikipedia", "wikipedia.org"),
    ("维基百科", "zh.wikipedia.org"),
    ("知乎", "zhihu.com"),
    ("zhihu", "zhihu.com"),
    ("bilibili", "bilibili.com"),
    ("哔哩哔哩", "bilibili.com"),
    ("微博", "weibo.com"),
    ("baidu", "baidu.com"),
    ("百度", "baidu.com"),
    ("淘宝", "taobao.com"),
    ("jd", "jd.com"),
    ("京东", "jd.com"),
    ("douban", "douban.com"),
    ("豆瓣", "douban.com"),
    ("stackoverflow", "stackoverflow.com"),
    ("stack overflow", "stackoverflow.com"),
    ("reddit", "reddit.com"),
    ("twitter", "x.com"),
    ("x", "x.com"),
    ("youtube", "youtube.com"),
    ("arxiv", "arxiv.org"),
    ("npm", "npmjs.com"),
    ("crates.io", "crates.io"),
    ("docs.rs", "docs.rs"),
    ("python", "python.org"),
    ("rust", "rust-lang.org"),
    ("mozilla", "mozilla.org"),
    ("cloudflare", "cloudflare.com"),
    ("openai", "openai.com"),
    ("anthropic", "anthropic.com"),
    ("deepseek", "deepseek.com"),
];

/// 直推时尝试的常见顶级域（按可能性排序）。
const COMMON_TLDS: &[(&str, u8)] = &[
    ("com", 55),
    ("org", 50),
    ("net", 45),
    ("io", 40),
    ("dev", 40),
    ("cn", 35),
];

/// 名称规范化：去空白、转小写、去常见后缀。
fn normalize(name: &str) -> String {
    let mut text = name.trim().to_ascii_lowercase();
    for suffix in [" 官网", "官网", " official site", " website"] {
        if let Some(stripped) = text.strip_suffix(suffix) {
            text = stripped.trim().to_owned();
        }
    }
    text
}

/// 直推：把名称当成域名主体，尝试常见 TLD。
///
/// 只在名称是"看起来像域名主体"时直推（ASCII、无空格、长度合理）；
/// 中文名不做直推（无法可靠音译 ✗）。
pub fn direct_candidates(name: &str) -> Vec<Candidate> {
    let normalized = normalize(name);
    if normalized.is_empty()
        || !normalized
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        return Vec::new();
    }
    let mut labels: Vec<&str> = normalized
        .split('.')
        .filter(|part| !part.is_empty())
        .collect();
    let already_has_tld = labels.len() >= 2;
    let host_base = if already_has_tld {
        labels.truncate(labels.len() - 1);
        labels.join(".")
    } else {
        normalized.clone()
    };
    if host_base.is_empty() || host_base.len() > 63 {
        return Vec::new();
    }
    let mut out = Vec::new();
    if already_has_tld {
        out.push(Candidate {
            url: format!("https://{normalized}"),
            score: 70,
            evidence: format!("名称本身看起来就是域名：{normalized}"),
            channel: "direct",
        });
        return out;
    }
    for (tld, score) in COMMON_TLDS {
        out.push(Candidate {
            url: format!("https://{host_base}.{tld}"),
            score: *score,
            evidence: format!("直推：{host_base} + .{tld}"),
            channel: "direct",
        });
    }
    out
}

/// 查内置映射表（大小写与空白不敏感）。
pub fn lookup_builtin(name: &str) -> Option<Candidate> {
    let normalized = normalize(name);
    BUILTIN_SITES
        .iter()
        .find(|(key, _)| *key == normalized)
        .map(|(key, domain)| Candidate {
            url: format!("https://{domain}"),
            score: 95,
            evidence: format!("内置映射表：{key} -> {domain}"),
            channel: "builtin",
        })
}

/// L1 入口：内置表优先，其次直推；合并后按分数降序（同分保持确定性顺序）。
pub fn resolve_l1(name: &str, max_candidates: usize) -> Vec<Candidate> {
    let mut out = Vec::new();
    if let Some(hit) = lookup_builtin(name) {
        out.push(hit);
    }
    out.extend(direct_candidates(name));
    merge(vec![out], max_candidates)
}

/// 合并多层候选：分数降序 → **全局**按 url 去重（保留高分那条）→ 截断。
///
/// 为什么必须全局去重：排序后同一 url 未必相邻（例如 wikipedia：内置 .org 95、
/// 直推 .com 55、直推 .org 50），dedup_by 只比较相邻元素会漏掉重复。
pub fn merge(groups: Vec<Vec<Candidate>>, max_candidates: usize) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = groups.into_iter().flatten().collect();
    // 稳定排序：同分时保持输入顺序（L1 先于 L3，内置先于直推）。
    out.sort_by_key(|candidate| std::cmp::Reverse(candidate.score));
    let mut seen = std::collections::HashSet::new();
    out.retain(|candidate| seen.insert(candidate.url.clone()));
    out.truncate(max_candidates);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_hits_first_with_high_score() {
        let out = resolve_l1("知乎", 5);
        assert_eq!(out[0].url, "https://zhihu.com");
        assert_eq!(out[0].channel, "builtin");
        assert_eq!(out[0].score, 95);
    }

    #[test]
    fn builtin_is_case_and_space_insensitive() {
        assert_eq!(
            resolve_l1("Hacker News", 3)[0].url,
            "https://news.ycombinator.com"
        );
        assert_eq!(resolve_l1("  GitHub  ", 3)[0].url, "https://github.com");
    }

    #[test]
    fn ascii_name_gets_direct_candidates() {
        let out = resolve_l1("example", 3);
        assert!(out.iter().all(|c| c.channel == "direct"));
        assert_eq!(out[0].url, "https://example.com");
        assert!(out.len() <= 3);
    }

    #[test]
    fn chinese_name_never_guessed() {
        let out = resolve_l1("某个不知名的中文站", 5);
        assert!(out.is_empty(), "中文名不直推、不猜：{out:?}");
    }

    #[test]
    fn url_like_name_returns_single_candidate() {
        let out = resolve_l1("example.org", 5);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "https://example.org");
    }

    #[test]
    fn no_duplicate_urls_when_builtin_and_direct_overlap() {
        let out = resolve_l1("github", 10);
        let before = out.len();
        let mut urls: Vec<String> = out.iter().map(|c| c.url.clone()).collect();
        urls.sort_unstable();
        urls.dedup();
        assert_eq!(before, urls.len(), "候选 URL 必须唯一：{out:?}");
        assert_eq!(out[0].url, "https://github.com");
        assert_eq!(out[0].channel, "builtin", "保留高分的内置那一条");
    }

    #[test]
    fn no_duplicate_urls_even_when_duplicates_are_not_adjacent() {
        // wikipedia：内置 wikipedia.org(95)、直推 wikipedia.com(55)、直推 wikipedia.org(50)。
        // 排序后两条 .org 被 .com 隔开 —— dedup_by 只比较相邻元素，会漏掉这条重复。
        let out = resolve_l1("wikipedia", 10);
        let mut urls: Vec<String> = out.iter().map(|c| c.url.clone()).collect();
        let before = urls.len();
        urls.sort_unstable();
        urls.dedup();
        assert_eq!(before, urls.len(), "候选 URL 必须唯一：{out:?}");
        assert_eq!(out[0].url, "https://wikipedia.org");
        assert_eq!(out[0].channel, "builtin", "保留高分的内置那一条");
        assert_eq!(
            out.iter()
                .filter(|c| c.url == "https://wikipedia.org")
                .count(),
            1,
            "内置命中不能与直推的同一域名并存：{out:?}"
        );
    }

    #[test]
    fn deterministic_order_for_same_score() {
        let a = resolve_l1("example", 10);
        let b = resolve_l1("example", 10);
        assert_eq!(a, b, "同样输入必须同样输出");
    }
}
