//! L3 通道（ADR-0006）：免 key 公共只读 API —— Wikidata **P856（官方网站）**。
//!
//! 为什么是 Wikidata：无需 key、无商业后端、许可明确（CC0），且 P856 是"官方网站"的
//! 结构化声明（不是搜出来的猜测）。
//!
//! 诚实边界：
//! - **只读官方号**：只把 P856 的值当候选，不做字符串相似度猜补 ✗；
//! - **每次请求都过 politeness**（robots + 限速 + 429 退避），被 robots 拦下**不请求**，
//!   失败原因如实回传（"是 robots 拦的"）；
//! - 网络失败 / 限流 / JSON 解析失败都返回**显式失败**（调用方据此降级），绝不静默返回空；
//! - 证据里写清 Q 号与标签，调用方可复核。

use serde_json::Value;

use super::Candidate;
use crate::politeness::PoliteFetcher;

/// L3 候选的置信分：低于内置映射表（95，人工核对），高于域名直推（≤70）。
pub const SCORE: u8 = 90;

/// 默认搜索端点（占位符：query / lang / limit）。
pub const DEFAULT_SEARCH_URL: &str = "https://www.wikidata.org/w/api.php?action=wbsearchentities&type=item&format=json&language={lang}&uselang={lang}&limit={limit}&search={query}";

/// 默认实体端点（占位符：id）。
pub const DEFAULT_ENTITIES_URL: &str = "https://www.wikidata.org/w/api.php?action=wbgetentities&format=json&formatversion=2&props=claims&ids={id}";

/// L3 配置（端点可注入，测试因此能用本地 fixture 而不碰外网）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L3Config {
    /// 搜索端点模板。
    pub search_url: String,
    /// 实体端点模板。
    pub entities_url: String,
    /// 语言（zh / en …）。
    pub language: String,
    /// 搜索返回条数上限（取第一条命中）。
    pub limit: usize,
}

impl Default for L3Config {
    fn default() -> Self {
        Self {
            search_url: DEFAULT_SEARCH_URL.to_owned(),
            entities_url: DEFAULT_ENTITIES_URL.to_owned(),
            language: "zh".to_owned(),
            limit: 3,
        }
    }
}

/// L3 未能给出候选的原因（**都要能说清"卡在哪一步"**）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum L3Failure {
    /// 搜索请求失败（含 robots 拦截：message 里会写 "robots.txt disallows …"）。
    SearchUnavailable {
        /// 信封码的字符串形式（BLOCKED / NETWORK / TIMEOUT …）。
        code: String,
        /// 可读原因。
        message: String,
    },
    /// 搜索响应不是预期的 JSON 形状。
    SearchParse(String),
    /// 搜索有响应但没有命中项（名称在 Wikidata 里查不到）。
    NoMatch,
    /// 实体请求失败。
    EntitiesUnavailable {
        /// 信封码的字符串形式。
        code: String,
        /// 可读原因。
        message: String,
    },
    /// 实体响应不是预期的 JSON 形状。
    EntitiesParse(String),
    /// 命中了实体，但它没有 P856（没有声明官方网站）。
    NoOfficialSite,
}

impl L3Failure {
    /// 机器可读的原因码（写进 actionable.reason 的前缀）。
    pub fn reason(&self) -> &'static str {
        match self {
            Self::SearchUnavailable { .. } => "l3-search-unavailable",
            Self::SearchParse(_) => "l3-search-parse-failed",
            Self::NoMatch => "l3-no-match",
            Self::EntitiesUnavailable { .. } => "l3-entities-unavailable",
            Self::EntitiesParse(_) => "l3-entities-parse-failed",
            Self::NoOfficialSite => "l3-no-official-site",
        }
    }

    /// 人类可读的一句话（写进 actionable.detected）。
    pub fn describe(&self) -> String {
        match self {
            Self::SearchUnavailable { code, message } => {
                format!("Wikidata 搜索请求失败（{code}）：{message}")
            }
            Self::SearchParse(detail) => format!("Wikidata 搜索响应无法解析：{detail}"),
            Self::NoMatch => "Wikidata 搜索没有命中任何条目".to_owned(),
            Self::EntitiesUnavailable { code, message } => {
                format!("Wikidata 实体请求失败（{code}）：{message}")
            }
            Self::EntitiesParse(detail) => format!("Wikidata 实体响应无法解析：{detail}"),
            Self::NoOfficialSite => "命中的 Wikidata 条目没有 P856（官方网站）声明".to_owned(),
        }
    }
}

/// L3 查询结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L3Outcome {
    /// 候选（通常 0 或 1 条：P856 一般只有一个值）。
    pub candidates: Vec<Candidate>,
    /// 命中的实体 Q 号（有则写进证据）。
    pub qid: Option<String>,
    /// 失败原因（成功时为 None）。
    pub failure: Option<L3Failure>,
}

impl L3Outcome {
    fn failed(failure: L3Failure) -> Self {
        Self {
            candidates: Vec::new(),
            qid: None,
            failure: Some(failure),
        }
    }
}

/// 查询名称对应的官方网站（L3）。
///
/// 两步请求：先 wbsearchentities 找实体，再 wbgetentities 取 P856。
/// 两步都走传入的 fetcher（生产 = 礼貌取回器；测试 = 指向本地 fixture 的同一个类型）。
pub async fn resolve(name: &str, config: &L3Config, fetcher: &PoliteFetcher) -> L3Outcome {
    let search_url = render(
        &config.search_url,
        &[
            ("query", percent_encode(name)),
            ("lang", config.language.clone()),
            ("limit", config.limit.max(1).to_string()),
        ],
    );
    let search_body = match body_of(fetcher, &search_url).await {
        Ok(text) => text,
        Err((code, message)) => {
            return L3Outcome::failed(L3Failure::SearchUnavailable { code, message });
        }
    };
    let (qid, label) = match parse_search(&search_body) {
        Ok(Some(hit)) => hit,
        Ok(None) => return L3Outcome::failed(L3Failure::NoMatch),
        Err(detail) => return L3Outcome::failed(L3Failure::SearchParse(detail)),
    };

    let entities_url = render(&config.entities_url, &[("id", qid.clone())]);
    let entities_body = match body_of(fetcher, &entities_url).await {
        Ok(text) => text,
        Err((code, message)) => {
            return L3Outcome {
                candidates: Vec::new(),
                qid: Some(qid),
                failure: Some(L3Failure::EntitiesUnavailable { code, message }),
            };
        }
    };
    match parse_official_site(&entities_body, &qid) {
        Ok(Some(url)) => L3Outcome {
            candidates: vec![Candidate {
                url,
                score: SCORE,
                evidence: format!(
                    "Wikidata P856：{}（{qid}）",
                    label.unwrap_or_else(|| qid.clone())
                ),
                channel: "public-api",
            }],
            qid: Some(qid),
            failure: None,
        },
        Ok(None) => L3Outcome {
            candidates: Vec::new(),
            qid: Some(qid),
            failure: Some(L3Failure::NoOfficialSite),
        },
        Err(detail) => L3Outcome {
            candidates: Vec::new(),
            qid: Some(qid),
            failure: Some(L3Failure::EntitiesParse(detail)),
        },
    }
}

/// 取回一个端点并返回正文；失败时给出（码字符串, 可读原因）。
async fn body_of(fetcher: &PoliteFetcher, url: &str) -> Result<String, (String, String)> {
    let envelope = fetcher.fetch(url).await;
    match envelope.data {
        Some(outcome) => outcome.fetched.content.text.ok_or_else(|| {
            (
                "PARSE".to_owned(),
                format!("响应没有可解析的文本正文：{url}"),
            )
        }),
        None => Err((envelope.code.as_str().to_owned(), envelope.error)),
    }
}

/// 解析 wbsearchentities 响应，返回（Q 号, 标签）。
fn parse_search(body: &str) -> Result<Option<(String, Option<String>)>, String> {
    let value: Value = serde_json::from_str(body).map_err(|error| format!("{error}"))?;
    let Some(items) = value.get("search").and_then(Value::as_array) else {
        return Err("缺少 search 数组".to_owned());
    };
    for item in items {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        if !id.starts_with('Q') {
            continue;
        }
        let label = item.get("label").and_then(Value::as_str).map(str::to_owned);
        return Ok(Some((id.to_owned(), label)));
    }
    Ok(None)
}

/// 从 wbgetentities 响应里取 P856 的第一个值。
fn parse_official_site(body: &str, qid: &str) -> Result<Option<String>, String> {
    let value: Value = serde_json::from_str(body).map_err(|error| format!("{error}"))?;
    let Some(entity) = value.get("entities").and_then(|entities| entities.get(qid)) else {
        return Err(format!("缺少 entities[{qid}]"));
    };
    let claims = entity
        .get("claims")
        .and_then(Value::as_object)
        .ok_or_else(|| "缺少 claims 对象".to_owned())?;
    let Some(p856) = claims.get("P856").and_then(Value::as_array) else {
        return Ok(None);
    };
    for claim in p856 {
        let Some(url) = claim
            .get("mainsnak")
            .and_then(|snak| snak.get("datavalue"))
            .and_then(|data| data.get("value"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let normalized = normalize_site_url(url);
        if !normalized.is_empty() {
            return Ok(Some(normalized));
        }
    }
    Ok(None)
}

/// 规范化官方网站 URL：https、去尾斜杠（与 L1 的候选格式一致）。
fn normalize_site_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let with_scheme = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    with_scheme
        .replacen("http://", "https://", 1)
        .trim_end_matches('/')
        .to_owned()
}

/// 把 {key} 占位符替换成已编码的值。
fn render(template: &str, pairs: &[(&str, String)]) -> String {
    let mut out = template.to_owned();
    for (key, value) in pairs {
        out = out.replace(&format!("{{{key}}}"), value);
    }
    out
}

/// 查询串组件编码（空格 -> %20，不做 + 号）。
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
    fn percent_encode_keeps_unreserved_and_escapes_cjk() {
        assert_eq!(percent_encode("知乎"), "%E7%9F%A5%E4%B9%8E");
        assert_eq!(percent_encode("hacker news"), "hacker%20news");
        assert_eq!(percent_encode("crates.io"), "crates.io");
    }

    #[test]
    fn render_replaces_all_placeholders() {
        let out = render(
            "https://x/api?q={query}&l={lang}&l={lang}&n={limit}",
            &[
                ("query", percent_encode("知乎")),
                ("lang", "zh".to_owned()),
                ("limit", "3".to_owned()),
            ],
        );
        assert_eq!(out, "https://x/api?q=%E7%9F%A5%E4%B9%8E&l=zh&l=zh&n=3");
    }

    #[test]
    fn parse_search_takes_first_qid_with_label() {
        let body = r#"{"search":[{"id":"Q123","label":"某站","description":"d"}]}"#;
        assert_eq!(
            parse_search(body).expect("ok"),
            Some(("Q123".to_owned(), Some("某站".to_owned())))
        );
    }

    #[test]
    fn parse_search_rejects_non_qid_entries() {
        let body = r#"{"search":[{"id":"P1"}]}"#;
        assert_eq!(parse_search(body).expect("ok"), None);
    }

    #[test]
    fn parse_search_reports_missing_array() {
        assert!(parse_search(r#"{"ok":1}"#).is_err());
        assert!(parse_search("not json").is_err());
    }

    #[test]
    fn parse_official_site_reads_p856_and_normalizes() {
        let body = r#"{"entities":{"Q1":{"claims":{"P856":[{"mainsnak":{"datavalue":{"value":"http://example.com/"}}}]}}}}"#;
        assert_eq!(
            parse_official_site(body, "Q1").expect("ok"),
            Some("https://example.com".to_owned())
        );
    }

    #[test]
    fn parse_official_site_without_p856_is_none() {
        let body = r#"{"entities":{"Q1":{"claims":{"P31":[]}}}}"#;
        assert_eq!(parse_official_site(body, "Q1").expect("ok"), None);
    }

    #[test]
    fn parse_official_site_reports_missing_entity() {
        assert!(parse_official_site(r#"{"entities":{}}"#, "Q1").is_err());
        assert!(parse_official_site("not json", "Q1").is_err());
    }

    #[test]
    fn failure_reasons_are_stable() {
        assert_eq!(L3Failure::NoMatch.reason(), "l3-no-match");
        assert_eq!(L3Failure::NoOfficialSite.reason(), "l3-no-official-site");
        assert_eq!(
            L3Failure::SearchUnavailable {
                code: "BLOCKED".to_owned(),
                message: "robots.txt disallows …".to_owned()
            }
            .reason(),
            "l3-search-unavailable"
        );
    }
}
