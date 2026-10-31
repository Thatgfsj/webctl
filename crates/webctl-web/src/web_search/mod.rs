//! 全网搜索（0.2.2，议题 #199 / #200）：引擎模型、随机抖动、Bing SERP 适配。
//!
//! 设计边界见 `docs/ROADMAP-0.2.2.md`：
//! - 默认 Bing；结果按**不可信页面内容**处理；本层只解析，不抓正文；
//! - 每个 HTTP 请求前加 **50–500ms 随机抖动**（用户要求）；
//! - robots 默认不拦 SERP（用户决策：搜索优先、合规其次），该事实写进调用方 `notes`；
//!   `strict_robots=true` 时由 `PoliteFetcher` 拦截（禁则 0 请求）；
//! - 失败不伪装成功：被拦 / 空结果 / 超时都返回带 `degraded_from` 的信封。

pub mod bing;

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 默认返回条数。
pub const DEFAULT_MAX_RESULTS: usize = 10;
/// 单次最多返回条数。
pub const MAX_RESULTS_LIMIT: usize = 30;
/// 抖动下界（毫秒）。
pub const JITTER_MIN_MS: u64 = 50;
/// 抖动上界（毫秒）。
pub const JITTER_MAX_MS: u64 = 500;

/// 支持的搜索引擎（`engines` 参数）。
///
/// M1 只接线 `Bing`；其余名字可解析但会如实报“尚未接线（M3）”，
/// 而不是静默换成别的引擎。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchEngine {
    /// Bing（默认）。
    Bing,
    /// Mojeek。
    Mojeek,
    /// Marginalia。
    Marginalia,
    /// 百度。
    Baidu,
    /// 360 搜索。
    Sogou,
    /// Yandex。
    Yandex,
}

impl SearchEngine {
    /// 默认引擎。
    pub const DEFAULT: Self = Self::Bing;

    /// 线上名字。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bing => "bing",
            Self::Mojeek => "mojeek",
            Self::Marginalia => "marginalia",
            Self::Baidu => "baidu",
            Self::Sogou => "360",
            Self::Yandex => "yandex",
        }
    }

    /// 解析名字（大小写不敏感）。
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "bing" => Some(Self::Bing),
            "mojeek" => Some(Self::Mojeek),
            "marginalia" => Some(Self::Marginalia),
            "baidu" => Some(Self::Baidu),
            "360" | "so" | "sogou" => Some(Self::Sogou),
            "yandex" => Some(Self::Yandex),
            _ => None,
        }
    }

    /// 全部合法名字。
    pub fn all() -> &'static [Self] {
        &[
            Self::Bing,
            Self::Mojeek,
            Self::Marginalia,
            Self::Baidu,
            Self::Sogou,
            Self::Yandex,
        ]
    }
}

/// 随机抖动配置（每次请求前 sleep）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Jitter {
    /// 下界（毫秒）。
    pub min_ms: u64,
    /// 上界（毫秒）。
    pub max_ms: u64,
    /// 随机种子（测试可注入固定值以获得可复现序列）。
    pub seed: u64,
}

impl Default for Jitter {
    fn default() -> Self {
        Self {
            min_ms: JITTER_MIN_MS,
            max_ms: JITTER_MAX_MS,
            seed: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9E37_79B9_7F4A_7C15),
        }
    }
}

impl Jitter {
    /// 固定种子的抖动（测试用）。
    pub fn seeded(seed: u64) -> Self {
        Self {
            seed,
            ..Self::default()
        }
    }

    /// 给定盐值（通常是对 URL 的哈希）算一次延迟，落在 `[min_ms, max_ms]`。
    pub fn delay_ms(&self, salt: u64) -> u64 {
        let lo = self.min_ms.min(self.max_ms);
        let hi = self.max_ms.max(self.min_ms);
        if hi == lo {
            return lo;
        }
        let span = hi - lo + 1;
        lo + xorshift64(self.seed ^ salt) % span
    }
}

/// 64 位 xorshift：无依赖、可复现。
fn xorshift64(mut value: u64) -> u64 {
    if value == 0 {
        value = 0x9E37_79B9_7F4A_7C15;
    }
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    value
}

/// 对 URL 做稳定哈希（FNV-1a），供抖动取盐。
pub fn hash_salt(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// 一条搜索结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    /// 名次（从 1 开始，按引擎返回顺序）。
    pub rank: u32,
    /// 引擎名。
    pub engine: String,
    /// 标题。
    pub title: String,
    /// 原始链接（只保留 http(s)）。
    pub url: String,
    /// 展示用域名/路径（引擎给的 cite）。
    pub display_url: String,
    /// 摘要片段。
    pub snippet: String,
    /// 命中该结果的引擎（M3 聚合用；M1 恒为单元素）。
    pub engines: Vec<String>,
}

/// 相关主题 / 相关搜索。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedTopic {
    /// 标题。
    pub title: String,
    /// 链接（只保留 http(s)）。
    pub url: String,
}

/// 引用来源（摘要/答案卡片用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLink {
    /// 标题。
    pub title: String,
    /// 链接。
    pub url: String,
}

/// 引擎生成的 LLM 摘要（M2；M1 恒为 `None`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmSummary {
    /// 摘要正文（**不可信生成内容**）。
    pub text: String,
    /// 摘要引用的来源。
    pub sources: Vec<SourceLink>,
    /// 生成引擎。
    pub engine: String,
    /// 信任级别（恒为 untrusted）。
    pub trust: String,
    /// 是否必须回原始链接核对（恒为 true）。
    pub must_visit_sources: bool,
}

/// 单个引擎的执行状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineStatus {
    /// 引擎名。
    pub engine: String,
    /// 状态：`hit` / `empty` / `blocked` / `timeout` / `unavailable` / `unsupported`。
    pub status: String,
    /// 结果条数。
    pub count: usize,
    /// 耗时（毫秒）。
    pub elapsed_ms: u64,
    /// 说明（失败原因等）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `web_search` 的完整结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchReport {
    /// 查询词。
    pub query: String,
    /// 合并后的结果（M1 单引擎）。
    pub results: Vec<SearchHit>,
    /// 逐引擎状态。
    pub engines: Vec<EngineStatus>,
    /// LLM 摘要（M2 起可能非空；必须回源核对）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_summary: Option<LlmSummary>,
    /// 相关主题 / 相关搜索。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<RelatedTopic>,
    /// 合规/口径说明（必读）。
    pub notes: Vec<String>,
}

impl SearchReport {
    /// 空结果（失败/降级时仍有完整骨架）。
    pub fn empty(query: &str) -> Self {
        Self {
            query: query.to_owned(),
            results: Vec::new(),
            engines: Vec::new(),
            llm_summary: None,
            related: Vec::new(),
            notes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_names_round_trip() {
        for engine in SearchEngine::all() {
            assert_eq!(SearchEngine::parse(engine.as_str()), Some(*engine));
        }
        assert_eq!(SearchEngine::parse("BING"), Some(SearchEngine::Bing));
        assert_eq!(SearchEngine::parse("so"), Some(SearchEngine::Sogou));
        assert_eq!(SearchEngine::parse("google"), None);
    }

    #[test]
    fn jitter_stays_within_bounds_and_is_reproducible() {
        let jitter = Jitter::seeded(42);
        for salt in 0..200u64 {
            let delay = jitter.delay_ms(salt);
            // 直接钉死用户口径的 50–500ms（不拿同一组常量自证）。
            assert!((50..=500).contains(&delay), "{delay}");
            assert_eq!(delay, jitter.delay_ms(salt));
        }
    }

    #[test]
    fn jitter_uses_the_whole_range() {
        let jitter = Jitter::seeded(7);
        let mut seen_low = false;
        let mut seen_high = false;
        for salt in 0..2_000u64 {
            let delay = jitter.delay_ms(salt);
            if delay < 120 {
                seen_low = true;
            }
            if delay > 430 {
                seen_high = true;
            }
        }
        assert!(seen_low && seen_high);
    }

    #[test]
    fn salt_hash_is_stable() {
        assert_eq!(
            hash_salt("https://www.bing.com/search?q=a"),
            hash_salt("https://www.bing.com/search?q=a")
        );
        assert_ne!(hash_salt("a"), hash_salt("b"));
    }
}
