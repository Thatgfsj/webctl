//! L4 通道（ADR-0006）：候选域名的 robots.txt **Sitemap 声明**。
//!
//! 定位语境：直推候选（name -> name.com 这类猜测）**是不是一个真实站点**？
//! 站点自己声明的 sitemap 是一个可信信号，但它**只证明"域名在运营"**，
//! 不证明"这就是用户说的那个站"——所以本通道**只报告声明**，不据此提升为可靠定位，
//! 也不去抓 sitemap 内容做页面抽取（那是 #147/#153 的通道抽取，与 L4 无关）。
//!
//! 礼貌性：robots.txt 走 [`PoliteFetcher::robots_rules`]（同一节流与缓存），不额外开请求通道。

use serde::Serialize;

use super::Candidate;
use crate::politeness::PoliteFetcher;

/// 一个候选域名的 sitemap 声明探测结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SitemapReport {
    /// 被探测的候选 URL（原样回填，便于调用方对齐）。
    pub candidate_url: String,
    /// 该候选的 origin（robots.txt 所在位置）。
    pub origin: String,
    /// 声明状态。
    pub status: SitemapStatus,
    /// `Sitemap:` 声明清单（保持文件顺序；未声明时为空）。
    pub sitemaps: Vec<String>,
}

/// 声明状态（三态必须分开：没有声明 ≠ 拿不到 robots.txt）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SitemapStatus {
    /// robots.txt 可读且声明了至少一条 sitemap。
    Declared,
    /// robots.txt 可读但没有任何 sitemap 声明。
    None,
    /// robots.txt 拿不到（404、非 2xx、非文本、或站点不可达）——**不能**据此说"没声明"。
    Unavailable,
}

/// 探测单个候选域名。
pub async fn probe(candidate_url: &str, fetcher: &PoliteFetcher) -> SitemapReport {
    let Ok(url) = url::Url::parse(candidate_url) else {
        return SitemapReport {
            candidate_url: candidate_url.to_owned(),
            origin: String::new(),
            status: SitemapStatus::Unavailable,
            sitemaps: Vec::new(),
        };
    };
    let origin = url.origin().ascii_serialization();
    match fetcher.robots_rules(&url).await {
        Some(rules) => {
            let sitemaps: Vec<String> = rules.sitemaps().to_vec();
            SitemapReport {
                candidate_url: candidate_url.to_owned(),
                origin,
                status: if sitemaps.is_empty() {
                    SitemapStatus::None
                } else {
                    SitemapStatus::Declared
                },
                sitemaps,
            }
        }
        None => SitemapReport {
            candidate_url: candidate_url.to_owned(),
            origin,
            status: SitemapStatus::Unavailable,
            sitemaps: Vec::new(),
        },
    }
}

/// 探测前 `max` 个候选（按传入顺序，通常是分数降序）。
///
/// 上限存在的理由：每个候选要多发一次 robots.txt 请求，必须给调用方的
/// 单次工具预算留余量（默认工具预算 60s、单请求 30s）。
pub async fn probe_top(
    candidates: &[Candidate],
    max: usize,
    fetcher: &PoliteFetcher,
) -> Vec<SitemapReport> {
    let mut out = Vec::new();
    for candidate in candidates.iter().take(max.max(1)) {
        out.push(probe(&candidate.url, fetcher).await);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&SitemapStatus::Declared).expect("json"),
            "\"declared\""
        );
        assert_eq!(
            serde_json::to_string(&SitemapStatus::None).expect("json"),
            "\"none\""
        );
        assert_eq!(
            serde_json::to_string(&SitemapStatus::Unavailable).expect("json"),
            "\"unavailable\""
        );
    }
}
