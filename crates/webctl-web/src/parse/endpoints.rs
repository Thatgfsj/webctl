//! 页面内 API 端点线索发现（议题 #116 / EPIC #113）。
//!
//! **只读页面源码，绝不探测**：本模块不持有任何网络能力，只从内联脚本与内嵌 JSON
//! 里把页面**自己写着**的候选端点取出来。调用方拿到的是线索，不是已经取到的数据；
//! 要不要请求、请求哪一个，由调用方自己决定（并自行遵守 robots 与礼貌策略）。
//!
//! 提取来源：
//! - fetch("…") / axios.get("…") 这类显式调用；
//! - 引号里的 /api/…、/graphql、/rest/…、/v1/…、/_next/data/… 字面量；
//! - 内嵌 JSON 的 {"url":"/api/…"} 字段（也认 endpoint / api / uri）；
//! - Next.js __NEXT_DATA__ 的 buildId + page，据此合成
//!   /_next/data/<buildId>/<page>.json。
//!
//! 归一化与分组：
//! - 相对路径按基准 URL（页面最终 URL 或调用方给的 base）解析成绝对 URL；
//! - 与基准 URL **同源**的进 same_origin，其余显式绝对地址进 cross_origin；
//! - 没有基准 URL 时，相对路径进 unresolved（不猜），绝对地址进 cross_origin
//!   且 same_origin = null（无法核对，不能声称同源）。
//!
//! 上限：候选数默认 32、单条 URL 默认 1024 字符；超限只截断输出并置
//! truncated=true，**不**发请求、不保存超长字符串。
//!
//! 实现约束：零子进程、零 unsafe；扫描是线性的，JSON 遍历有深度与节点预算。

use std::collections::BTreeSet;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use super::embedded::{scan_json_literal, script_blocks};

/// 单次调用最多返回多少条候选端点（防止畸形页面把结果撑爆）。
pub const MAX_API_CANDIDATES: usize = 32;

/// 单条候选端点 URL 的字符上限；超过则丢弃并标记 truncated。
pub const MAX_API_URL_CHARS: usize = 1024;

/// 硬上限：超过这个长度的原始字符串不再复制/归一化，只置 truncated。
///
/// 这是防畸形输入的内存护栏，不是对外承诺的 URL 语义上限（那个是
/// [`MAX_API_URL_CHARS`]）。
const MAX_API_RAW_CHARS: usize = 8192;

/// 输出里固定携带的定位说明：这是线索，不是已经取到的数据。
pub const API_DISCOVERY_NOTE: &str = "hints only: these URLs were written in the page itself; webctl made no request to them. Fetch and verify them yourself (respect robots.txt and rate limits) before trusting any data.";

/// 候选端点抽取选项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiDiscoveryOptions {
    /// 最多返回多少条（默认 MAX_API_CANDIDATES）。
    pub max_candidates: usize,
    /// 单条 URL 字符上限（默认 MAX_API_URL_CHARS）。
    pub max_url_chars: usize,
}

impl Default for ApiDiscoveryOptions {
    fn default() -> Self {
        Self {
            max_candidates: MAX_API_CANDIDATES,
            max_url_chars: MAX_API_URL_CHARS,
        }
    }
}

/// 一条候选端点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiEndpoint {
    /// 归一化后的绝对 URL；没有基准 URL 或解析失败时为 null。
    pub url: Option<String>,
    /// 页面里原样的字符串（超长项直接丢弃）。
    pub raw: String,
    /// 来源：fetch / axios / json-url / string-literal / next-data。
    pub source: String,
    /// 类别：rest / graphql / next-data。
    pub kind: String,
    /// 与基准 URL 是否同源；没有基准 URL 时为 null（无法核对）。
    pub same_origin: Option<bool>,
}

/// 本次生效的上限（回显给调用方，便于解释 truncated）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiDiscoveryLimits {
    /// 返回条数上限。
    pub max_candidates: usize,
    /// 单条 URL 字符上限。
    pub max_url_chars: usize,
}

/// API 端点线索报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiDiscoveryReport {
    /// 人类可读结论（含扫描与截断情况）。
    pub conclusion: String,
    /// 归一化用的基准 URL（页面最终 URL 或调用方给的 base）。
    pub base_url: Option<String>,
    /// 与基准 URL 同源的候选。
    pub same_origin: Vec<ApiEndpoint>,
    /// 显式绝对地址：与基准 URL 不同源，或没有基准 URL 无法核对。
    pub cross_origin: Vec<ApiEndpoint>,
    /// 相对路径但没有基准 URL（或解析失败），不猜。
    pub unresolved: Vec<ApiEndpoint>,
    /// 看过的 script 标签数（透明度：证明确实没有，而不是没看）。
    pub scanned_scripts: usize,
    /// 去重后见到的候选数（含因上限未返回的）。
    pub candidates: usize,
    /// 实际返回的条数。
    pub returned: usize,
    /// 是否因候选数或 URL 长度上限发生截断。
    pub truncated: bool,
    /// 因上限被丢弃的候选数。
    pub dropped: usize,
    /// 本次生效的上限。
    pub limits: ApiDiscoveryLimits,
    /// 固定说明：这是线索，不是已经取到的数据；本工具没有发任何请求。
    pub note: String,
}

/// 从 HTML 的内联脚本与内嵌 JSON 中发现 API 端点线索。
///
/// 纯函数：不联网、不执行脚本、不读写磁盘；同一输入总是得到同一报告。
pub fn discover_api_endpoints(
    html: &str,
    base: Option<&Url>,
    options: &ApiDiscoveryOptions,
) -> ApiDiscoveryReport {
    let mut sink = Sink::new(base, options);
    for (_open_at, _attrs, body) in script_blocks(html) {
        sink.scanned_scripts += 1;
        if body.trim().is_empty() {
            continue;
        }
        scan_calls(body, &mut sink);
        scan_json_url_fields(body, &mut sink);
        scan_api_literals(body, &mut sink);
        scan_embedded_json(body, &mut sink);
        scan_next_data_assignments(body, &mut sink);
    }
    sink.finish()
}

// ---------------------------------------------------------------- 扫描器

fn fetch_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)\bfetch\s*\(\s*["']([^"'\r\n]{1,2048})["']"#).expect("static fetch regex")
    })
}

fn axios_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)\baxios(?:\s*\.\s*(?:get|post|put|patch|delete|head|options))?\s*\(\s*["']([^"'\r\n]{1,2048})["']"#,
        )
        .expect("static axios regex")
    })
}

fn url_field_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)(?:["'](?:url|endpoint|api|uri)["']|\b(?:url|endpoint|api|uri)\b)\s*:\s*["']([^"'\r\n]{1,2048})["']"#,
        )
        .expect("static url field regex")
    })
}

fn api_literal_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)["'](/(?:api|graphql|rest|wp-json|ajax|_next/data|v[0-9]+)(?:[/?#][^"'\s<>]*)?)["']"#,
        )
        .expect("static api literal regex")
    })
}

fn scan_calls(body: &str, sink: &mut Sink<'_>) {
    for caps in fetch_regex().captures_iter(body) {
        if let Some(raw) = caps.get(1) {
            sink.push(raw.as_str(), "fetch", false);
        }
    }
    for caps in axios_regex().captures_iter(body) {
        if let Some(raw) = caps.get(1) {
            sink.push(raw.as_str(), "axios", false);
        }
    }
}

fn scan_json_url_fields(body: &str, sink: &mut Sink<'_>) {
    for caps in url_field_regex().captures_iter(body) {
        if let Some(raw) = caps.get(1) {
            sink.push(raw.as_str(), "json-url", true);
        }
    }
}

fn scan_api_literals(body: &str, sink: &mut Sink<'_>) {
    for caps in api_literal_regex().captures_iter(body) {
        if let Some(raw) = caps.get(1) {
            sink.push(raw.as_str(), "string-literal", false);
        }
    }
}

/// 解析整块内嵌 JSON（script[type=json] / JSON-LD / __NEXT_DATA__）。
fn scan_embedded_json(body: &str, sink: &mut Sink<'_>) {
    let trimmed = body.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return;
    };
    let mut budget = 8192usize;
    walk_json(&value, 0, &mut budget, sink);
    if let Some(object) = value.as_object() {
        next_data_from_object(object, sink);
    }
}

/// 解析 __NEXT_DATA__ = {…}; 这种内联赋值（不是独立 JSON script）。
fn scan_next_data_assignments(body: &str, sink: &mut Sink<'_>) {
    const MARKER: &str = "__NEXT_DATA__";
    let mut cursor = 0usize;
    while let Some(offset) = body[cursor..].find(MARKER) {
        let at = cursor + offset;
        let mut j = at + MARKER.len();
        let bytes = body.as_bytes();
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'=' {
            j += 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if let Some(end) = scan_json_literal(body, j, 64)
                && let Ok(Value::Object(object)) = serde_json::from_str::<Value>(&body[j..end])
            {
                next_data_from_object(&object, sink);
            }
        }
        cursor = at + MARKER.len();
    }
}

/// 递归找 url / endpoint / api / uri 字段；深度与节点数都有预算。
fn walk_json(value: &Value, depth: usize, budget: &mut usize, sink: &mut Sink<'_>) {
    if depth > 16 || *budget == 0 {
        return;
    }
    *budget -= 1;
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                if matches!(key.as_str(), "url" | "endpoint" | "api" | "uri")
                    && let Some(raw) = item.as_str()
                {
                    sink.push(raw, "json-url", true);
                }
                walk_json(item, depth + 1, budget, sink);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_json(item, depth + 1, budget, sink);
            }
        }
        _ => {}
    }
}

/// Next.js：buildId + page → /_next/data/<buildId>/<route>.json。
fn next_data_from_object(object: &serde_json::Map<String, Value>, sink: &mut Sink<'_>) {
    let Some(build) = object.get("buildId").and_then(Value::as_str) else {
        return;
    };
    if build.is_empty()
        || build.len() > 128
        || !build
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return;
    }
    let page = object
        .get("page")
        .and_then(Value::as_str)
        .filter(|page| page.starts_with('/') && !page.contains('['));
    let route = match page {
        Some(page) => page
            .split(['?', '#'])
            .next()
            .unwrap_or(page)
            .trim_matches('/')
            .to_owned(),
        None => match sink.base {
            Some(base) => base.path().trim_matches('/').to_owned(),
            None => return,
        },
    };
    let path = if route.is_empty() {
        format!("/_next/data/{build}/index.json")
    } else {
        format!("/_next/data/{build}/{route}.json")
    };
    sink.push(&path, "next-data", false);
}

// ---------------------------------------------------------------- 收集器

struct Sink<'a> {
    base: Option<&'a Url>,
    options: &'a ApiDiscoveryOptions,
    seen: BTreeSet<String>,
    same_origin: Vec<ApiEndpoint>,
    cross_origin: Vec<ApiEndpoint>,
    unresolved: Vec<ApiEndpoint>,
    scanned_scripts: usize,
    candidates: usize,
    dropped: usize,
    truncated: bool,
}

impl<'a> Sink<'a> {
    fn new(base: Option<&'a Url>, options: &'a ApiDiscoveryOptions) -> Self {
        Self {
            base,
            options,
            seen: BTreeSet::new(),
            same_origin: Vec::new(),
            cross_origin: Vec::new(),
            unresolved: Vec::new(),
            scanned_scripts: 0,
            candidates: 0,
            dropped: 0,
            truncated: false,
        }
    }

    fn returned(&self) -> usize {
        self.same_origin.len() + self.cross_origin.len() + self.unresolved.len()
    }

    fn push(&mut self, raw: &str, source: &str, require_api_shape: bool) {
        if raw.chars().count() > MAX_API_RAW_CHARS {
            self.candidates += 1;
            self.dropped += 1;
            self.truncated = true;
            return;
        }
        let raw = normalize_raw(raw);
        if !is_plausible(&raw) {
            return;
        }
        if require_api_shape && !looks_like_api_path(&raw) {
            return;
        }
        if raw.chars().count() > self.options.max_url_chars {
            if self.seen.insert(format!("raw:{raw}")) {
                self.candidates += 1;
                self.dropped += 1;
            }
            self.truncated = true;
            return;
        }
        let resolved = resolve(&raw, self.base);
        let key = resolved
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| raw.clone());
        if !self.seen.insert(key) {
            return;
        }
        self.candidates += 1;
        if self.returned() >= self.options.max_candidates {
            self.truncated = true;
            self.dropped += 1;
            return;
        }
        let same = match (self.base, resolved.as_ref()) {
            (Some(base), Some(url)) => Some(crate::transport::same_origin(base, url)),
            _ => None,
        };
        let kind = kind_of(resolved.as_ref().map_or(raw.as_str(), Url::as_str));
        let endpoint = ApiEndpoint {
            url: resolved.as_ref().map(ToString::to_string),
            raw,
            source: source.to_owned(),
            kind,
            same_origin: same,
        };
        match (resolved, same) {
            (Some(_), Some(true)) => self.same_origin.push(endpoint),
            (Some(_), _) => self.cross_origin.push(endpoint),
            (None, _) => self.unresolved.push(endpoint),
        }
    }

    fn finish(self) -> ApiDiscoveryReport {
        let returned = self.returned();
        let limits = ApiDiscoveryLimits {
            max_candidates: self.options.max_candidates,
            max_url_chars: self.options.max_url_chars,
        };
        let conclusion = if self.candidates == 0 {
            format!(
                "scanned {} script tag(s); no API endpoint hints found (valid empty result, no requests made)",
                self.scanned_scripts
            )
        } else {
            let cut = if self.truncated {
                format!(
                    "; output truncated by limits ({} candidate(s) dropped)",
                    self.dropped
                )
            } else {
                String::new()
            };
            format!(
                "scanned {} script tag(s); found {} API candidate(s): {} same-origin, {} cross-origin, {} unresolved; {} returned{}; no requests were made (hints only)",
                self.scanned_scripts,
                self.candidates,
                self.same_origin.len(),
                self.cross_origin.len(),
                self.unresolved.len(),
                returned,
                cut
            )
        };
        ApiDiscoveryReport {
            conclusion,
            base_url: self.base.map(ToString::to_string),
            same_origin: self.same_origin,
            cross_origin: self.cross_origin,
            unresolved: self.unresolved,
            scanned_scripts: self.scanned_scripts,
            candidates: self.candidates,
            returned,
            truncated: self.truncated,
            dropped: self.dropped,
            limits,
            note: API_DISCOVERY_NOTE.to_owned(),
        }
    }
}

// ---------------------------------------------------------------- 归一化

fn normalize_raw(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.contains("\\/") {
        trimmed.replace("\\/", "/")
    } else {
        trimmed.to_owned()
    }
}

fn is_plausible(raw: &str) -> bool {
    if raw.is_empty() {
        return false;
    }
    if raw.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return false;
    }
    if raw.starts_with('#') {
        return false;
    }
    let lower = raw.to_ascii_lowercase();
    !["javascript:", "data:", "mailto:", "tel:", "blob:", "about:"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn looks_like_api_path(raw: &str) -> bool {
    let path = raw
        .split(['?', '#'])
        .next()
        .unwrap_or(raw)
        .to_ascii_lowercase();
    const PREFIXES: [&str; 6] = [
        "/api",
        "/graphql",
        "/rest",
        "/wp-json",
        "/ajax",
        "/_next/data",
    ];
    const CONTAINS: [&str; 5] = ["/api/", "/graphql/", "/rest/", "/wp-json/", "/_next/data/"];
    if PREFIXES.iter().any(|prefix| {
        path == *prefix
            || path.strip_prefix(prefix).is_some_and(|rest| {
                rest.starts_with('/') || rest.starts_with('?') || rest.starts_with('#')
            })
    }) || CONTAINS.iter().any(|marker| path.contains(marker))
    {
        return true;
    }
    path.split('/').any(|segment| {
        segment.len() > 1
            && segment.starts_with('v')
            && segment[1..].bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn kind_of(raw_url: &str) -> String {
    let path = raw_url
        .split(['?', '#'])
        .next()
        .unwrap_or(raw_url)
        .to_ascii_lowercase();
    if path.contains("/_next/data/") {
        "next-data".to_owned()
    } else if path == "/graphql" || path.ends_with("/graphql") || path.contains("/graphql/") {
        "graphql".to_owned()
    } else {
        "rest".to_owned()
    }
}

fn resolve(raw: &str, base: Option<&Url>) -> Option<Url> {
    let parsed = Url::parse(raw)
        .ok()
        .or_else(|| base.and_then(|base| base.join(raw).ok()))
        .or_else(|| {
            raw.strip_prefix("//")
                .and_then(|rest| Url::parse(&format!("https://{rest}")).ok())
        })?;
    matches!(parsed.scheme(), "http" | "https").then_some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(html: &str, base: Option<&str>) -> ApiDiscoveryReport {
        let base = base.map(|raw| Url::parse(raw).expect("valid base"));
        discover_api_endpoints(html, base.as_ref(), &ApiDiscoveryOptions::default())
    }

    fn find<'a>(list: &'a [ApiEndpoint], source: &str) -> &'a ApiEndpoint {
        list.iter()
            .find(|endpoint| endpoint.source == source)
            .unwrap_or_else(|| panic!("missing source {source}: {list:?}"))
    }

    /// Next.js：__NEXT_DATA__ 的 buildId + page 合成 data 端点。
    #[test]
    fn next_data_build_id_becomes_data_url() {
        let html = r#"<script id="__NEXT_DATA__" type="application/json">{"buildId":"abc123","page":"/posts/7"}</script>"#;
        let report = report(html, Some("https://example.test/posts/7"));
        let endpoint = find(&report.same_origin, "next-data");
        assert_eq!(
            endpoint.url.as_deref(),
            Some("https://example.test/_next/data/abc123/posts/7.json")
        );
        assert_eq!(endpoint.kind, "next-data");
    }

    /// GraphQL：fetch("/graphql") 归一化成绝对 URL，kind=graphql。
    #[test]
    fn graphql_fetch_is_detected() {
        let html = r#"<script>fetch("/graphql", {method: "POST"});</script>"#;
        let report = report(html, Some("https://example.test/app"));
        let endpoint = find(&report.same_origin, "fetch");
        assert_eq!(
            endpoint.url.as_deref(),
            Some("https://example.test/graphql")
        );
        assert_eq!(endpoint.kind, "graphql");
        assert_eq!(endpoint.same_origin, Some(true));
    }

    /// 相对路径按基准 URL 归一化（含相对当前目录的写法）。
    #[test]
    fn relative_paths_are_absolutized() {
        let html = r#"<script>fetch("api/items?page=2");</script>"#;
        let report = report(html, Some("https://example.test/app/page"));
        let endpoint = find(&report.same_origin, "fetch");
        assert_eq!(
            endpoint.url.as_deref(),
            Some("https://example.test/app/api/items?page=2")
        );
    }

    /// 跨域显式地址单独列出，不混进同源。
    #[test]
    fn cross_origin_is_separated() {
        let html = r#"<script>fetch("https://api.other.test/v1/items");</script>"#;
        let report = report(html, Some("https://example.test/"));
        assert!(report.same_origin.is_empty());
        assert_eq!(report.cross_origin.len(), 1);
        assert_eq!(report.cross_origin[0].same_origin, Some(false));
    }

    /// 内嵌 JSON 的 url 字段（含转义斜杠）。
    #[test]
    fn json_url_field_is_detected() {
        let html = r#"<script>window.__STATE__={"url":"\/api\/items"};</script>"#;
        let report = report(html, Some("https://example.test/"));
        let endpoint = find(&report.same_origin, "json-url");
        assert_eq!(
            endpoint.url.as_deref(),
            Some("https://example.test/api/items")
        );
    }

    /// 没有基准 URL 时相对路径进 unresolved，不猜绝对地址。
    #[test]
    fn no_base_leaves_relative_unresolved() {
        let html = r#"<script>fetch("/api/items");</script>"#;
        let report = report(html, None);
        assert!(report.same_origin.is_empty());
        assert!(report.cross_origin.is_empty());
        assert_eq!(report.unresolved.len(), 1);
        assert!(report.unresolved[0].url.is_none());
        assert_eq!(report.unresolved[0].same_origin, None);
    }

    /// 候选数上限：截断且保留前 N 条。
    #[test]
    fn candidate_limit_truncates() {
        let mut html = String::new();
        for index in 0..10 {
            html.push_str(&format!(r#"<script>fetch("/api/items/{index}")</script>"#));
        }
        let base = Url::parse("https://example.test/").expect("base");
        let options = ApiDiscoveryOptions {
            max_candidates: 4,
            ..ApiDiscoveryOptions::default()
        };
        let report = discover_api_endpoints(&html, Some(&base), &options);
        assert_eq!(report.returned, 4);
        assert_eq!(report.candidates, 10);
        assert_eq!(report.dropped, 6);
        assert!(report.truncated);
    }

    /// URL 长度上限：超长项丢弃并置 truncated，不保存超长字符串。
    #[test]
    fn oversized_url_is_dropped() {
        let long = "a".repeat(2000);
        let html = format!(r#"<script>fetch("/api/{long}")</script>"#);
        let base = Url::parse("https://example.test/").expect("base");
        let options = ApiDiscoveryOptions {
            max_url_chars: 100,
            ..ApiDiscoveryOptions::default()
        };
        let report = discover_api_endpoints(&html, Some(&base), &options);
        assert!(report.truncated);
        assert_eq!(report.returned, 0);
        assert_eq!(report.candidates, 1);
        assert_eq!(report.dropped, 1);
    }

    /// 同一 URL 被 fetch 与普通字面量同时命中时只报一次（先命中的来源生效）。
    #[test]
    fn duplicates_are_collapsed() {
        let html = r#"<script>fetch("/api/items"); const path = "/api/items";</script>"#;
        let report = report(html, Some("https://example.test/"));
        assert_eq!(report.returned, 1);
        assert_eq!(report.candidates, 1);
        assert_eq!(report.same_origin[0].source, "fetch");
    }

    /// 超过硬上限的原始串不再复制，仍然如实置 truncated。
    #[test]
    fn absurdly_long_literal_marks_truncated() {
        let long = "a".repeat(9000);
        let html = format!(r#"<script>const p = "/api/{long}";</script>"#);
        let base = Url::parse("https://example.test/").expect("base");
        let report = discover_api_endpoints(&html, Some(&base), &ApiDiscoveryOptions::default());
        assert!(report.truncated);
        assert_eq!(report.returned, 0);
        assert_eq!(report.candidates, 1);
        assert_eq!(report.dropped, 1);
    }

    /// 页面没有任何线索是合法空结果，不是失败。
    #[test]
    fn no_candidates_is_valid_empty_result() {
        let report = report("<html><body><p>纯静态</p></body></html>", None);
        assert_eq!(report.candidates, 0);
        assert_eq!(report.returned, 0);
        assert!(!report.truncated);
        assert!(report.conclusion.contains("valid empty result"));
    }
}
