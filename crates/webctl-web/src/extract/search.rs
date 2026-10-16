//! 文本 / 正则搜索（EXTRACT-08、09、17）。
//!
//! - 文本模式：查询串按字面量匹配（内部转义为正则，偏移与大小写语义仍明确）；
//! - 正则模式：直接用 regex crate 的线性时间引擎，天然免疫指数回溯，
//!   再叠加模式长度与编译尺寸上限，超限即拒（BLOCKED）；
//! - 每条命中带字节区间、行/列、命中串与前后上下文；
//! - 非法正则 → code=PARSE；零命中 → ok=true + 空数组 + 可读结论。

use std::time::Instant;

use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use webctl_core::{Envelope, ErrorCode, Trust};

use crate::parse::elapsed_ms;

use super::context::{head_chars, tail_chars};

/// search 一次调用的选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOptions {
    /// 前后上下文字符数。
    pub context_chars: usize,
    /// 最多返回多少条命中；超出即截断并标 LIMIT。
    pub max_results: usize,
    /// matched + before + after 的总字符预算；超出即截断并标 LIMIT。
    pub char_budget: usize,
    /// 是否大小写不敏感。
    pub case_insensitive: bool,
    /// true 表示 query 是正则；false 表示按字面量搜索。
    pub regex: bool,
    /// 模式字节数上限；超过直接 BLOCKED。
    pub max_pattern_len: usize,
    /// 墙钟预算（毫秒）；超时返回 TIMEOUT。
    pub max_elapsed_ms: u64,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            context_chars: 40,
            max_results: 50,
            char_budget: 8_000,
            case_insensitive: false,
            regex: false,
            max_pattern_len: 512,
            max_elapsed_ms: 5_000,
        }
    }
}

/// 一条搜索命中。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchMatch {
    /// 命中序号（0 起）。
    pub index: usize,
    /// 命中在文本中的起始字节偏移（UTF-8 边界）。
    pub start: usize,
    /// 结束字节偏移（不含）。
    pub end: usize,
    /// 1 起行号。
    pub line: usize,
    /// 1 起列号（按字符计）。
    pub column: usize,
    /// 命中串。
    pub matched: String,
    /// 命中前上下文（折叠空白）。
    pub before: String,
    /// 命中后上下文（折叠空白）。
    pub after: String,
}

/// search 的结果集。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    /// 查询串原文。
    pub query: String,
    /// 是否正则模式。
    pub regex: bool,
    /// 是否大小写不敏感。
    pub case_insensitive: bool,
    /// 命中。
    pub matches: Vec<SearchMatch>,
    /// 扫描到的命中数（受 max_results 限制）。
    pub observed_matches: usize,
    /// 是否因上限提前停止。
    pub more_matches: bool,
    /// 是否截断。
    pub truncated: bool,
    /// 截断原因。
    pub truncated_reasons: Vec<String>,
    /// 可读结论。
    pub conclusion: String,
}

/// 搜索文本，返回命中数、位置与上下文。
///
/// - 非法正则 → ok=false + code=PARSE；
/// - 超长模式 → ok=false + code=BLOCKED；
/// - 零命中 → ok=true + 空数组 + 结论；
/// - 超 max_results / char_budget → ok=true + code=LIMIT + 非空 degraded_from。
pub fn search(text: &str, query: &str, options: &SearchOptions) -> Envelope<SearchResult> {
    let started = Instant::now();
    if query.is_empty() {
        return Envelope::fail(ErrorCode::Parse, "empty search query", elapsed_ms(started));
    }
    if query.len() > options.max_pattern_len {
        return Envelope::fail(
            ErrorCode::Blocked,
            format!(
                "search pattern rejected: {} bytes exceeds cap {} (refusing to compile an untrusted oversized pattern)",
                query.len(),
                options.max_pattern_len
            ),
            elapsed_ms(started),
        );
    }
    let pattern = if options.regex {
        query.to_owned()
    } else {
        regex::escape(query)
    };
    let compiled = match RegexBuilder::new(&pattern)
        .case_insensitive(options.case_insensitive)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
    {
        Ok(compiled) => compiled,
        Err(error) => {
            return Envelope::fail(
                ErrorCode::Parse,
                format!("invalid regular expression {query:?}: {error}"),
                elapsed_ms(started),
            );
        }
    };

    let line_starts = line_starts(text);
    let mut result = SearchResult {
        query: query.to_owned(),
        regex: options.regex,
        case_insensitive: options.case_insensitive,
        matches: Vec::new(),
        observed_matches: 0,
        more_matches: false,
        truncated: false,
        truncated_reasons: Vec::new(),
        conclusion: String::new(),
    };
    let mut used_chars = 0usize;
    let mut reasons: Vec<String> = Vec::new();
    let mut more = false;

    for found in compiled.find_iter(text) {
        if elapsed_ms(started) >= options.max_elapsed_ms {
            return Envelope::fail(
                ErrorCode::Timeout,
                format!(
                    "search exceeded wall-clock budget of {} ms",
                    options.max_elapsed_ms
                ),
                elapsed_ms(started),
            );
        }
        result.observed_matches += 1;
        if result.matches.len() >= options.max_results {
            more = true;
            reasons.push("max-results".to_owned());
            break;
        }
        let before = crate::parse::html::collapse_ws(&tail_chars(
            &text[..found.start()],
            options.context_chars,
        ));
        let after = crate::parse::html::collapse_ws(&head_chars(
            &text[found.end()..],
            options.context_chars,
        ));
        let matched = found.as_str().to_owned();
        let cost = matched.chars().count() + before.chars().count() + after.chars().count();
        let remaining = options.char_budget.saturating_sub(used_chars);
        if cost > remaining {
            reasons.push("char-budget".to_owned());
            more = true;
            if remaining > 0 && result.matches.is_empty() {
                let (line, column) = line_column(&line_starts, text, found.start());
                result.matches.push(SearchMatch {
                    index: 0,
                    start: found.start(),
                    end: found.start() + char_boundary_len(&matched, remaining),
                    line,
                    column,
                    matched: head_chars(&matched, remaining),
                    before: String::new(),
                    after: String::new(),
                });
            }
            break;
        }
        used_chars += cost;
        let (line, column) = line_column(&line_starts, text, found.start());
        result.matches.push(SearchMatch {
            index: result.matches.len(),
            start: found.start(),
            end: found.end(),
            line,
            column,
            matched,
            before,
            after,
        });
    }

    if reasons.is_empty() {
        result.conclusion = if result.matches.is_empty() {
            format!(
                "query {query:?} matched 0 times; this is a valid empty result, not a parse failure"
            )
        } else {
            format!("query {query:?} matched {} time(s)", result.matches.len())
        };
        Envelope::ok(result, Trust::UntrustedPageContent, elapsed_ms(started))
    } else {
        result.truncated = true;
        result.more_matches = more;
        result.truncated_reasons = dedup(&reasons);
        result.conclusion = format!(
            "query {query:?} results truncated ({}) with {} match(es) returned",
            result.truncated_reasons.join(", "),
            result.matches.len()
        );
        let from = result.truncated_reasons.join(",");
        Envelope::degraded(result, from, ErrorCode::Limit, elapsed_ms(started))
    }
}

/// 每行起始字节偏移（0 起）。
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

/// 由字节偏移推 1 起行号与按字符计列号。
fn line_column(starts: &[usize], text: &str, offset: usize) -> (usize, usize) {
    let line = starts.partition_point(|start| *start <= offset).max(1);
    let line_start = starts[line - 1];
    let column = text[line_start..offset].chars().count() + 1;
    (line, column)
}

/// 将字符数换算成不超过该字符数的字节长度（用于截断后的 end 偏移）。
fn char_boundary_len(text: &str, max_chars: usize) -> usize {
    text.char_indices()
        .nth(max_chars)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
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
