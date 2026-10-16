//! 链接抽取与过滤（EXTRACT-12）。
//!
//! 解析能力完全复用 crate::parse::extract_links（a[href]/area[href]、base 解析、
//! rel 归一化），本模块只叠加**过滤**：按域名、按 rel、按 href 正则，以及
//! 内链/外链。过滤后零命中是合法空结果（ok=true），非法正则才是 PARSE。

use std::time::Instant;

use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use url::Url;
use webctl_core::{Envelope, ErrorCode, Trust};

use crate::parse::elapsed_ms;
use crate::parse::{Link, extract_links};

use super::ExtractOptions;

/// 链接过滤条件。多个条件同时给出时取交集（AND）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkFilter {
    /// 域名白名单：host 等于该值，或以 .value 结尾（大小写不敏感）。
    pub domains: Vec<String>,
    /// rel 白名单：链接 rel 与该列表有交集即保留（大小写不敏感）。
    pub rel: Vec<String>,
    /// href 正则过滤。
    pub pattern: Option<String>,
    /// 只保留与 base 同 host 的链接。
    pub internal_only: bool,
    /// 只保留与 base 不同 host 的链接（无法解析 host 的链接会被排除）。
    pub external_only: bool,
}

/// 过滤后的链接结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilteredLinks {
    /// 过滤后保留的链接（文档序）。
    pub links: Vec<Link>,
    /// 过滤前链接总数。
    pub observed: usize,
    /// 被过滤掉的数量。
    pub filtered_out: usize,
    /// 是否因 max_results 截断。
    pub truncated: bool,
    /// 截断原因。
    pub truncated_reasons: Vec<String>,
    /// 可读结论。
    pub conclusion: String,
}

/// 抽取并按条件过滤链接。
pub fn links(
    html: &str,
    base: Option<&str>,
    filter: &LinkFilter,
    options: &ExtractOptions,
) -> Envelope<FilteredLinks> {
    let started = Instant::now();
    let pattern = match filter.pattern.as_deref() {
        None => None,
        Some(raw) => {
            if raw.len() > options.max_selector_len {
                return Envelope::fail(
                    ErrorCode::Blocked,
                    format!(
                        "link pattern rejected: {} bytes exceeds cap {} (refusing to compile an untrusted oversized pattern)",
                        raw.len(),
                        options.max_selector_len
                    ),
                    elapsed_ms(started),
                );
            }
            match RegexBuilder::new(raw)
                .size_limit(1 << 20)
                .dfa_size_limit(1 << 20)
                .build()
            {
                Ok(compiled) => Some(compiled),
                Err(error) => {
                    return Envelope::fail(
                        ErrorCode::Parse,
                        format!("invalid link pattern {raw:?}: {error}"),
                        elapsed_ms(started),
                    );
                }
            }
        }
    };
    let base_host = base
        .and_then(|raw| Url::parse(raw).ok())
        .map(|url| url.host_str().unwrap_or_default().to_ascii_lowercase());

    let extracted = extract_links(html, base);
    if !extracted.ok {
        return Envelope::fail(extracted.code, extracted.error, elapsed_ms(started));
    }
    let all = extracted.data.unwrap_or_default();
    let observed = all.len();
    let mut kept: Vec<Link> = Vec::new();
    let mut filtered_out = 0usize;
    let mut reasons: Vec<String> = Vec::new();
    let mut more = false;

    for link in all {
        if elapsed_ms(started) >= options.max_elapsed_ms {
            return Envelope::fail(
                ErrorCode::Timeout,
                format!(
                    "link filtering exceeded wall-clock budget of {} ms",
                    options.max_elapsed_ms
                ),
                elapsed_ms(started),
            );
        }
        if !keep(&link, filter, pattern.as_ref(), base_host.as_deref()) {
            filtered_out += 1;
            continue;
        }
        if kept.len() >= options.max_results {
            more = true;
            reasons.push("max-results".to_owned());
            break;
        }
        kept.push(link);
    }

    let result = FilteredLinks {
        links: kept,
        observed,
        filtered_out,
        truncated: !reasons.is_empty(),
        truncated_reasons: dedup(&reasons),
        conclusion: String::new(),
    };
    if reasons.is_empty() {
        let mut result = result;
        result.conclusion = format!(
            "kept {} of {observed} link(s); {} filtered out",
            result.links.len(),
            result.filtered_out
        );
        Envelope::ok(result, Trust::UntrustedPageContent, elapsed_ms(started))
    } else {
        let mut result = result;
        result.conclusion = format!(
            "kept {} of {observed} link(s), truncated ({})",
            result.links.len(),
            result.truncated_reasons.join(", ")
        );
        let from = result.truncated_reasons.join(",");
        let _ = more;
        Envelope::degraded(result, from, ErrorCode::Limit, elapsed_ms(started))
    }
}

fn keep(
    link: &Link,
    filter: &LinkFilter,
    pattern: Option<&regex::Regex>,
    base_host: Option<&str>,
) -> bool {
    let host = Url::parse(&link.href)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase));
    if !filter.domains.is_empty() {
        let matched = host.as_deref().is_some_and(|host| {
            filter
                .domains
                .iter()
                .any(|domain| domain_matches(host, domain))
        });
        if !matched {
            return false;
        }
    }
    if !filter.rel.is_empty() {
        let matched = link.rel.iter().any(|token| {
            filter
                .rel
                .iter()
                .any(|want| want.eq_ignore_ascii_case(token))
        });
        if !matched {
            return false;
        }
    }
    if let Some(pattern) = pattern
        && !pattern.is_match(&link.href)
    {
        return false;
    }
    if filter.internal_only {
        if base_host.is_none() || host.as_deref() != base_host {
            return false;
        }
    }
    if filter.external_only {
        match (host.as_deref(), base_host) {
            (Some(host), Some(base)) if host != base => {}
            _ => return false,
        }
    }
    true
}

/// host 等于 domain，或以 .domain 结尾（大小写不敏感）。
fn domain_matches(host: &str, domain: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let domain = domain.trim().trim_start_matches('.').to_ascii_lowercase();
    !domain.is_empty() && (host == domain || host.ends_with(&format!(".{domain}")))
}

fn dedup(reasons: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for reason in reasons {
        if !out.contains(reason) {
            out.push(reason.clone());
        }
    }
    out
}
