//! 去广告规则层（0.2.3 M1，议题 #221）。
//!
//! 设计边界见 `docs/ROADMAP-0.2.3.md` §2.1：
//! - 规则是**声明式数据表** [`RULES`]，解析函数只把 SERP 字段交给 [`classify`]；
//! - 双层信号：显式广告标签（低置信 → 保留并打标）+ 引擎跳转域（高置信 → 剔除并计数）；
//! - **宁漏勿杀**：只有 host + path 前缀精确命中的跳转域才算高置信；域名相似不算；
//! - ASCII 标签按词边界匹配（`Ad` 命中 `Ad: ...`，但不命中 `Adapter` / `Advanced`）；
//! - 标签只在标题文本上匹配：展示 URL 里的 `/ad` 是路径而不是广告标签。

use url::Url;

/// 规则适用的搜索引擎。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdEngine {
    /// Bing。
    Bing,
    /// 百度。
    Baidu,
    /// 360 搜索。
    So360,
    /// 跨引擎通用的标签规则；`classify` 的入参不会用它。
    Any,
}

impl AdEngine {
    /// 从线上引擎名解析（与 `SearchEngine::as_str` 对齐）。
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "bing" => Some(Self::Bing),
            "baidu" => Some(Self::Baidu),
            "360" | "so" | "sogou" => Some(Self::So360),
            _ => None,
        }
    }

    /// 线上引擎名。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bing => "bing",
            Self::Baidu => "baidu",
            Self::So360 => "360",
            Self::Any => "any",
        }
    }

    fn applies_to(self, engine: Self) -> bool {
        self == Self::Any || self == engine
    }
}

/// 广告信号种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdKind {
    /// 显式广告标签文本（低置信）。
    Label,
    /// 引擎的广告跳转域（高置信）。
    RedirectHost,
}

/// 规则置信度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdConfidence {
    /// 高置信：剔除结果，并计入 `filtered_ads`。
    High,
    /// 低置信：保留结果，只标 `is_ad: true`。
    Low,
}

/// 一条声明式广告规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdRule {
    /// 适用引擎（`Any` 表示跨引擎通用）。
    pub engine: AdEngine,
    /// 信号种类。
    pub kind: AdKind,
    /// 匹配模式：标签文本，或 `host/path-prefix` 形式的跳转域。
    pub pattern: &'static str,
    /// 置信度。
    pub confidence: AdConfidence,
}

/// 规则表（唯一事实来源；改规则只改这里）。
///
/// 跳转域格式为 `host/path-prefix`：host 精确匹配或子域匹配，path 前缀区分大小写。
pub static RULES: &[AdRule] = &[
    // 跳转域：高置信，直接剔除。path 区分大小写是刻意的：
    // 360 的自然结果用 `/link`，广告用 `/Link`；不能把自然结果误杀。
    AdRule {
        engine: AdEngine::Bing,
        kind: AdKind::RedirectHost,
        pattern: "bing.com/ck/a",
        confidence: AdConfidence::High,
    },
    AdRule {
        engine: AdEngine::Baidu,
        kind: AdKind::RedirectHost,
        pattern: "baidu.com/baidu.php",
        confidence: AdConfidence::High,
    },
    AdRule {
        engine: AdEngine::So360,
        kind: AdKind::RedirectHost,
        pattern: "so.com/Link",
        confidence: AdConfidence::High,
    },
    // 显式标签：低置信，只打标不剔除。
    AdRule {
        engine: AdEngine::Any,
        kind: AdKind::Label,
        pattern: "广告",
        confidence: AdConfidence::Low,
    },
    AdRule {
        engine: AdEngine::Any,
        kind: AdKind::Label,
        pattern: "Ad",
        confidence: AdConfidence::Low,
    },
    AdRule {
        engine: AdEngine::Any,
        kind: AdKind::Label,
        pattern: "Sponsored",
        confidence: AdConfidence::Low,
    },
    AdRule {
        engine: AdEngine::Any,
        kind: AdKind::Label,
        pattern: "赞助商链接",
        confidence: AdConfidence::Low,
    },
    AdRule {
        engine: AdEngine::Any,
        kind: AdKind::Label,
        pattern: "商业推广",
        confidence: AdConfidence::Low,
    },
];

/// 一次判定的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdVerdict {
    /// 是否命中任一广告信号。
    pub is_ad: bool,
    /// 命中的最高置信度；`None` 表示未命中。
    pub confidence: Option<AdConfidence>,
}

impl AdVerdict {
    /// 未命中任何规则。
    pub const NATURAL: Self = Self {
        is_ad: false,
        confidence: None,
    };

    /// 是否应当剔除（只有高置信才剔除）。
    pub fn should_filter(&self) -> bool {
        self.is_ad && self.confidence == Some(AdConfidence::High)
    }
}

/// 对一条结果做广告判定。
///
/// - `engine`：该结果来自哪个引擎；
/// - `title_text`：结果标题（解析层可把标题旁的广告徽标文本一并传入）；
/// - `displayed_url`：引擎展示的域名/路径。M1 不用于标签匹配——展示 URL 的
///   `/ad` 是路径而非广告标签，扫描它只会增加误判；
/// - `real_url`：结果原始 `href`（**归一化之前**），跳转域规则用它判断。
///
/// 高置信优先：同一结果同时命中高/低置信规则时返回高置信。
pub fn classify(
    engine: AdEngine,
    title_text: &str,
    _displayed_url: &str,
    real_url: &str,
) -> AdVerdict {
    let mut verdict = AdVerdict::NATURAL;
    for rule in RULES {
        if !rule.engine.applies_to(engine) || !matches_rule(rule, title_text, real_url) {
            continue;
        }
        verdict.is_ad = true;
        if rule.confidence == AdConfidence::High {
            verdict.confidence = Some(AdConfidence::High);
            return verdict;
        }
        verdict.confidence = Some(AdConfidence::Low);
    }
    verdict
}

fn matches_rule(rule: &AdRule, title_text: &str, real_url: &str) -> bool {
    match rule.kind {
        AdKind::Label => matches_label(title_text, rule.pattern),
        AdKind::RedirectHost => matches_redirect_host(real_url, rule.pattern),
    }
}

/// 标签匹配：CJK 用子串；ASCII 用大小写不敏感的词边界匹配。
fn matches_label(text: &str, pattern: &str) -> bool {
    if !pattern.is_ascii() {
        return text.contains(pattern);
    }
    let haystack = text.to_ascii_lowercase();
    let needle = pattern.to_ascii_lowercase();
    let bytes = haystack.as_bytes();
    haystack.match_indices(needle.as_str()).any(|(start, _)| {
        let end = start + needle.len();
        let before_ok = start == 0 || !is_word_byte(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_word_byte(bytes[end]);
        before_ok && after_ok
    })
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// 跳转域匹配：scheme 必须是 http(s)，host 精确或子域匹配，path 前缀区分大小写。
fn matches_redirect_host(real_url: &str, pattern: &str) -> bool {
    let Ok(url) = Url::parse(real_url) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some((host_pattern, path_pattern)) = pattern.split_once('/') else {
        return false;
    };
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let host_matches = host == host_pattern || host.ends_with(&format!(".{host_pattern}"));
    host_matches && url.path().starts_with(&format!("/{path_pattern}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify_title(engine: AdEngine, title: &str) -> AdVerdict {
        classify(engine, title, "", "https://example.com/result")
    }

    #[test]
    fn rule_table_has_both_signal_layers_and_both_confidences() {
        assert!(RULES.iter().any(|rule| rule.kind == AdKind::Label));
        assert!(RULES.iter().any(|rule| rule.kind == AdKind::RedirectHost));
        assert!(
            RULES
                .iter()
                .any(|rule| rule.confidence == AdConfidence::High)
        );
        assert!(
            RULES
                .iter()
                .any(|rule| rule.confidence == AdConfidence::Low)
        );
        for rule in RULES {
            assert!(!rule.pattern.is_empty());
            if rule.kind == AdKind::RedirectHost {
                assert_eq!(rule.confidence, AdConfidence::High);
                assert!(rule.pattern.contains('/'), "跳转域必须带 path：{rule:?}");
            }
        }
    }

    #[test]
    fn every_label_is_low_confidence() {
        for rule in RULES.iter().filter(|rule| rule.kind == AdKind::Label) {
            assert_eq!(rule.confidence, AdConfidence::Low, "{rule:?}");
        }
    }

    #[test]
    fn detects_chinese_ad_labels() {
        for title in ["广告 - 某产品", "赞助商链接", "商业推广：某服务"] {
            let verdict = classify_title(AdEngine::Bing, title);
            assert!(verdict.is_ad, "{title}");
            assert_eq!(verdict.confidence, Some(AdConfidence::Low), "{title}");
            assert!(!verdict.should_filter(), "低置信不得剔除：{title}");
        }
    }

    #[test]
    fn detects_ascii_labels_case_insensitively_with_word_boundaries() {
        for title in ["Ad: save now", "Sponsored result", "ad · example"] {
            let verdict = classify_title(AdEngine::Baidu, title);
            assert!(verdict.is_ad, "{title}");
            assert_eq!(verdict.confidence, Some(AdConfidence::Low), "{title}");
        }
        for title in ["Adapter driver", "Advanced search", "Adobe reader"] {
            assert!(!classify_title(AdEngine::Baidu, title).is_ad, "{title}");
        }
    }

    #[test]
    fn display_url_path_is_not_a_label() {
        let verdict = classify(
            AdEngine::Bing,
            "Natural result",
            "example.com/ad",
            "https://example.com/ad",
        );
        assert!(!verdict.is_ad, "展示路径不得当成广告标签");
    }

    #[test]
    fn detects_redirect_hosts_with_subdomains() {
        let cases = [
            (
                AdEngine::Bing,
                "https://www.bing.com/ck/a?u=a1abc",
                AdConfidence::High,
            ),
            (
                AdEngine::Bing,
                "https://cn.bing.com/ck/a?u=x",
                AdConfidence::High,
            ),
            (
                AdEngine::Baidu,
                "https://www.baidu.com/baidu.php?url=x",
                AdConfidence::High,
            ),
            (
                AdEngine::So360,
                "https://www.so.com/Link?m=x",
                AdConfidence::High,
            ),
        ];
        for (engine, url, confidence) in cases {
            let verdict = classify(engine, "Natural title", "", url);
            assert!(verdict.is_ad, "{url}");
            assert_eq!(verdict.confidence, Some(confidence), "{url}");
            assert!(verdict.should_filter(), "{url}");
        }
    }

    #[test]
    fn redirect_host_rules_are_engine_scoped() {
        let url = "https://www.so.com/Link?m=x";
        assert!(!classify(AdEngine::Bing, "Natural", "", url).is_ad);
        assert!(!classify(AdEngine::Baidu, "Natural", "", url).is_ad);
    }

    #[test]
    fn similar_domains_and_wrong_case_are_not_high_confidence() {
        let cases = [
            (AdEngine::Bing, "https://evilbing.com/ck/a?u=x"),
            (AdEngine::Bing, "https://bing.com.evil.test/ck/a?u=x"),
            (AdEngine::So360, "https://www.so.com/link?m=x"),
            (AdEngine::Baidu, "https://www.baidu.com/link?url=x"),
            (AdEngine::Bing, "https://www.bing.com/search?q=ck/a"),
        ];
        for (engine, url) in cases {
            let verdict = classify(engine, "Natural result", "", url);
            assert!(!verdict.should_filter(), "{url}");
        }
    }

    #[test]
    fn non_http_redirect_is_ignored() {
        let verdict = classify(AdEngine::Bing, "Natural", "", "javascript:alert(1)");
        assert!(!verdict.is_ad);
        let verdict = classify(AdEngine::Bing, "Natural", "", "not a url");
        assert!(!verdict.is_ad);
    }

    #[test]
    fn high_confidence_wins_over_low() {
        let verdict = classify(
            AdEngine::Bing,
            "广告 - 某产品",
            "",
            "https://www.bing.com/ck/a?u=a1abc",
        );
        assert!(verdict.is_ad);
        assert_eq!(verdict.confidence, Some(AdConfidence::High));
        assert!(verdict.should_filter());
    }

    #[test]
    fn natural_result_is_not_ad() {
        let verdict = classify(
            AdEngine::Baidu,
            "Rust 程序设计语言",
            "doc.rust-lang.org",
            "https://doc.rust-lang.org/book/",
        );
        assert_eq!(verdict, AdVerdict::NATURAL);
        assert!(!verdict.should_filter());
    }

    #[test]
    fn engine_names_round_trip() {
        for (name, engine) in [
            ("bing", AdEngine::Bing),
            ("baidu", AdEngine::Baidu),
            ("360", AdEngine::So360),
        ] {
            assert_eq!(AdEngine::parse(name), Some(engine));
            assert_eq!(engine.as_str(), name);
        }
        assert_eq!(AdEngine::parse("sogou"), Some(AdEngine::So360));
        assert_eq!(AdEngine::parse("So"), Some(AdEngine::So360));
        assert_eq!(AdEngine::So360.as_str(), "360");
        assert_eq!(AdEngine::parse("google"), None);
    }
}
