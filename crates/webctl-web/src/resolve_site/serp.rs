//! L5 兜底通道（ADR-0006）：显式 opt-in 的 SERP 抓取。
//!
//! **默认关闭**：只有调用方在参数里写 allow_serp_fallback=true 才会走这里。
//! 理由（ADR-0006）：抓搜索结果页是最不可靠、最容易被拦、也最容易踩站点条款的一条路，
//! 因此它只能是"用户明确同意的最后一招"，不能是默认路径。
//!
//! 诚实边界：
//! - 结果**按不可靠来源处理**：分数低（≤50）、**不会跨过可靠阈值**，证据里写清"第 N 条"；
//! - 走 [`PoliteFetcher`]：robots 拦下**不请求**、429 退避、拦截页识别；
//! - 失败分三类并带码：请求失败（含 robots/BLOCKED）/ 解析失败 / 结果为空；
//! - 不伪装成"搜索结果可信"：调用方拿到的是候选，不是答案。

use url::Url;

use super::Candidate;
use crate::extract::{self, ExtractOptions, LinkFilter};
use crate::politeness::PoliteFetcher;

/// SERP 候选的置信分（低于 L1 直推的 ≤70，远低于可靠阈值 60）。
pub const SCORE: u8 = 45;

/// 默认 SERP 端点（占位符 query）。用 DuckDuckGo 的 HTML 版：无 key、无 JS。
pub const DEFAULT_SEARCH_URL: &str = "https://html.duckduckgo.com/html/?q={query}";

/// 默认查询后缀：站点名 + 官网，减少歧义。
pub const DEFAULT_QUERY_SUFFIX: &str = " 官网 official site";

/// L5 配置（端点/后缀可注入；测试因此指向本地 fixture）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerpConfig {
    /// SERP 端点模板（占位符 {query}）。
    pub search_url: String,
    /// 追加到站点名后的查询后缀。
    pub query_suffix: String,
    /// 最多取几条结果。
    pub max_results: usize,
    /// 引擎自身域名（结果里要过滤掉，避免把自己当候选）。
    pub engine_host: String,
}

impl Default for SerpConfig {
    fn default() -> Self {
        Self {
            search_url: DEFAULT_SEARCH_URL.to_owned(),
            query_suffix: DEFAULT_QUERY_SUFFIX.to_owned(),
            max_results: 5,
            engine_host: "duckduckgo.com".to_owned(),
        }
    }
}

/// L5 未能给出候选的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerpFailure {
    /// 请求失败（含 robots 拦截：message 会写 "robots.txt disallows …"）。
    Unavailable {
        /// 信封码字符串（BLOCKED / NETWORK / TIMEOUT …）。
        code: String,
        /// 可读原因。
        message: String,
    },
    /// 响应无法解析成链接。
    Parse(String),
    /// 页面可读但没有任何可用结果。
    Empty,
}

impl SerpFailure {
    /// 机器可读原因码。
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => "serp-unavailable",
            Self::Parse(_) => "serp-parse-failed",
            Self::Empty => "serp-empty",
        }
    }

    /// 人类可读的一句话。
    pub fn describe(&self) -> String {
        match self {
            Self::Unavailable { code, message } => {
                format!("SERP 请求失败（{code}）：{message}")
            }
            Self::Parse(detail) => format!("SERP 响应无法解析：{detail}"),
            Self::Empty => "SERP 页面没有任何可用结果".to_owned(),
        }
    }
}

/// L5 查询结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerpOutcome {
    /// 候选（按结果顺序，分数固定为 SCORE）。
    pub candidates: Vec<Candidate>,
    /// 失败原因（成功时为 None）。
    pub failure: Option<SerpFailure>,
}

/// 抓一次 SERP 并给出候选。
pub async fn search(name: &str, config: &SerpConfig, fetcher: &PoliteFetcher) -> SerpOutcome {
    let query = format!("{}{}", name.trim(), config.query_suffix);
    let url = config
        .search_url
        .replace("{query}", &percent_encode(&query));
    let envelope = fetcher.fetch(&url).await;
    let Some(fetched) = envelope.data else {
        return SerpOutcome {
            candidates: Vec::new(),
            failure: Some(SerpFailure::Unavailable {
                code: envelope.code.as_str().to_owned(),
                message: envelope.error,
            }),
        };
    };
    let Some(html) = fetched.fetched.content.text else {
        return SerpOutcome {
            candidates: Vec::new(),
            failure: Some(SerpFailure::Parse("响应没有可解析的文本正文".to_owned())),
        };
    };
    let base = fetched.fetched.final_url.clone();
    let options = ExtractOptions {
        max_results: config.max_results.max(1),
        ..ExtractOptions::default()
    };
    let links = extract::links(&html, Some(&base), &LinkFilter::default(), &options);
    let Some(filtered) = links.data else {
        return SerpOutcome {
            candidates: Vec::new(),
            failure: Some(SerpFailure::Parse(links.error)),
        };
    };
    let mut candidates = Vec::new();
    let mut seen = Vec::new();
    for link in filtered.links {
        let Some(target) = target_of(&link.href, &config.engine_host) else {
            continue;
        };
        if seen.iter().any(|known| known == &target) {
            continue;
        }
        seen.push(target.clone());
        let rank = candidates.len() + 1;
        let title = if link.text.trim().is_empty() {
            target.clone()
        } else {
            link.text.trim().to_owned()
        };
        candidates.push(Candidate {
            url: target,
            score: SCORE,
            evidence: format!("SERP 第 {rank} 条：{title}"),
            channel: "serp-fallback",
        });
        if candidates.len() >= config.max_results.max(1) {
            break;
        }
    }
    if candidates.is_empty() {
        return SerpOutcome {
            candidates,
            failure: Some(SerpFailure::Empty),
        };
    }
    SerpOutcome {
        candidates,
        failure: None,
    }
}

/// 把一条结果链接还原成目标站点 URL；引擎自身域名（含其跳转链接）一律过滤。
fn target_of(href: &str, engine_host: &str) -> Option<String> {
    let url = Url::parse(href).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    // 引擎跳转链接：目标在 uddg / url 查询参数里（DuckDuckGo 是 uddg）。
    for key in ["uddg", "url", "q"] {
        let Some(value) = url
            .query_pairs()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.into_owned())
        else {
            continue;
        };
        if let Ok(target) = Url::parse(&value)
            && matches!(target.scheme(), "http" | "https")
            && !host_matches(target.host_str().unwrap_or_default(), engine_host)
        {
            return Some(normalize(&target));
        }
    }
    if host_matches(url.host_str().unwrap_or_default(), engine_host) {
        return None;
    }
    Some(normalize(&url))
}

/// host 是否属于引擎域名（等于或以 .engine 结尾）。
fn host_matches(host: &str, engine_host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let engine = engine_host.to_ascii_lowercase();
    host == engine || host.ends_with(&format!(".{engine}"))
}

/// 规范化候选 URL：https、去尾斜杠（与 L1/L3 的候选格式一致）。
fn normalize(url: &Url) -> String {
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
    fn percent_encode_escapes_space_and_cjk() {
        assert_eq!(
            percent_encode("知乎 官网"),
            "%E7%9F%A5%E4%B9%8E%20%E5%AE%98%E7%BD%91"
        );
    }

    #[test]
    fn engine_own_links_are_filtered() {
        assert_eq!(
            target_of("https://duckduckgo.com/about", "duckduckgo.com"),
            None
        );
        assert_eq!(
            target_of("https://html.duckduckgo.com/html/?q=x", "duckduckgo.com"),
            None
        );
    }

    #[test]
    fn redirect_links_are_unwrapped() {
        let href = "https://duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2F&rut=x";
        assert_eq!(
            target_of(href, "duckduckgo.com"),
            Some("https://example.com".to_owned())
        );
    }

    #[test]
    fn plain_links_are_normalized_to_https_without_trailing_slash() {
        assert_eq!(
            target_of("http://example.com/", "duckduckgo.com"),
            Some("https://example.com".to_owned())
        );
        assert_eq!(
            target_of("https://example.org/path/", "duckduckgo.com"),
            Some("https://example.org/path".to_owned())
        );
    }

    #[test]
    fn non_http_links_are_rejected() {
        assert_eq!(target_of("javascript:void(0)", "duckduckgo.com"), None);
        assert_eq!(target_of("mailto:a@b.test", "duckduckgo.com"), None);
    }

    #[test]
    fn failure_reasons_are_stable() {
        assert_eq!(SerpFailure::Empty.reason(), "serp-empty");
        assert_eq!(
            SerpFailure::Unavailable {
                code: "BLOCKED".to_owned(),
                message: "robots.txt disallows …".to_owned()
            }
            .reason(),
            "serp-unavailable"
        );
    }
}
