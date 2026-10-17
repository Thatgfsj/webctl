//! 内嵌结构化数据抽取（议题 #114 / EPIC #113）。
//!
//! **零 JS 执行**：只把 HTML 里**已经存在**的结构化载荷取出来，不解释、不执行页面脚本。
//! 覆盖四类容器：
//! 1. `<script type="application/ld+json">`（JSON-LD，可同页多块、可为数组）；
//! 2. `<script type="application/json">`（含 `id` 名，例如 `__NEXT_DATA__`）；
//! 3. 内联状态赋值：`window.__NUXT__=`、`window.__INITIAL_STATE__=`、`__APOLLO_STATE__=` 等**对象/数组字面量**；
//! 4. Inertia 的 `data-page="…"`（HTML 实体转义后的 JSON）。
//!
//! 语义（与 `parse/mod.rs` 的硬约束一致）：
//! - 一条都没找到 → `ok=true` + `code=OK` + 空数组：这是**合法的零结果**，不是解析失败；
//! - 找到且至少一条解析成功 → `ok=true` + `code=OK`；解析失败的条目逐条带 `parse_error`；
//! - 找到但**全部**解析失败 → `ok=false` + `code=PARSE`（错误信息里列出每个失败来源）；
//! - 载荷超过字节上限 → `truncated=true` 且**不**尝试解析（不假装拿到完整数据）。
//!
//! 实现约束：零子进程、零 unsafe；扫描是**线性 + 深度受限**的，不递归、不建树。

use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use webctl_core::{Envelope, ErrorCode, Trust};

use super::elapsed_ms;

/// 单次调用最多返回多少条内嵌载荷（防止一个畸形页面把结果撑爆）。
pub const MAX_EMBEDDED_ENTRIES: usize = 32;

/// 单条载荷的字节上限；超过则标记 `truncated` 且不解析。
pub const MAX_EMBEDDED_BYTES: usize = 256 * 1024;

/// 字面量扫描的嵌套深度上限（防畸形输入把扫描器拖爆）。
pub const MAX_LITERAL_DEPTH: usize = 64;

/// 抽取选项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddedOptions {
    /// 最多返回多少条。
    pub max_entries: usize,
    /// 单条载荷字节上限。
    pub max_entry_bytes: usize,
    /// 是否抽取内联状态赋值（`window.__NUXT__=` 这类）。关掉可只取 JSON-LD 与 `script[type=json]`。
    pub include_inline_state: bool,
}

impl Default for EmbeddedOptions {
    fn default() -> Self {
        Self {
            max_entries: MAX_EMBEDDED_ENTRIES,
            max_entry_bytes: MAX_EMBEDDED_BYTES,
            include_inline_state: true,
        }
    }
}

/// 一条内嵌载荷。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddedEntry {
    /// 来源类型：`ld+json` / `script-json` / `inline-state` / `data-page`。
    pub source: String,
    /// 来源名（`script` 的 `id`，或内联状态变量名 / Inertia 的组件名）。
    pub name: Option<String>,
    /// 载荷的 UTF-8 字节数（截断时为**原始长度**）。
    pub bytes: usize,
    /// 是否因超限被截断；截断时不解析。
    pub truncated: bool,
    /// 解析成功时的 JSON 值。
    pub value: Option<Value>,
    /// 解析失败的原因（`truncated` 时为 None）。
    pub parse_error: Option<String>,
}

/// 抽取报告。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddedReport {
    /// 人类可读结论（含每个失败来源）。
    pub conclusion: String,
    /// 看过的 `script` 标签数（透明度：便于判断"确实没有"而不是"没看"）。
    pub scanned_scripts: usize,
    /// 按文档序返回的载荷。
    pub entries: Vec<EmbeddedEntry>,
    /// 解析失败的条数。
    pub failed: usize,
}

/// 从 HTML 中抽取内嵌结构化数据。
pub fn extract_embedded(html: &str, options: &EmbeddedOptions) -> Envelope<EmbeddedReport> {
    let started = Instant::now();
    let mut entries: Vec<EmbeddedEntry> = Vec::new();
    let mut scanned_scripts = 0usize;

    // 1) <script …>…</script>：JSON-LD 与 application/json
    for (_open_at, attrs, body) in script_blocks(html) {
        scanned_scripts += 1;
        let (kind, type_value) = classify_script(attrs);
        let Some(source) = kind else { continue };
        let name = attr_value(attrs, "id").or_else(|| type_value.clone());
        push_entry(&mut entries, options, source, name, body);
        if entries.len() >= options.max_entries {
            break;
        }
    }

    // 2) data-page（Inertia）：HTML 实体转义后的 JSON
    if entries.len() < options.max_entries {
        for raw in attr_values(html, "data-page") {
            let decoded = unescape_entities(&raw);
            push_entry(&mut entries, options, "data-page", None, &decoded);
            if entries.len() >= options.max_entries {
                break;
            }
        }
    }

    // 3) 内联状态赋值
    //
    // 变量表里既有 `window.__INITIAL_STATE__` 也有 `__INITIAL_STATE__`，同一处赋值会被两个
    // 模式同时命中 —— 必须按**匹配区间**去重，否则同一份数据会返回两遍。
    if options.include_inline_state && entries.len() < options.max_entries {
        let mut consumed: Vec<(usize, usize)> = Vec::new();
        for var in INLINE_STATE_VARS {
            if entries.len() >= options.max_entries {
                break;
            }
            for (at, literal) in inline_state_literals(html, var) {
                let span = (at, at + var.len());
                if consumed.iter().any(|(s, e)| at >= *s && at < *e) {
                    continue;
                }
                consumed.push(span);
                push_entry(
                    &mut entries,
                    options,
                    "inline-state",
                    Some(var.to_owned()),
                    &literal,
                );
                if entries.len() >= options.max_entries {
                    break;
                }
            }
        }
    }

    let failed = entries.iter().filter(|e| e.parse_error.is_some()).count();
    let parsed = entries.iter().filter(|e| e.value.is_some()).count();

    let report = EmbeddedReport {
        conclusion: conclusion(scanned_scripts, &entries, parsed, failed),
        scanned_scripts,
        entries,
        failed,
    };

    // 找到容器但一条也没解析成功 → 这是真正的 PARSE（不是"零结果"）。
    if report.entries.is_empty() {
        return Envelope::ok(report, Trust::UntrustedPageContent, elapsed_ms(started));
    }
    if parsed == 0 && failed > 0 {
        let detail: Vec<String> = report
            .entries
            .iter()
            .filter_map(|e| e.parse_error.as_ref().map(|m| format!("{}: {m}", e.source)))
            .collect();
        return Envelope::fail(
            ErrorCode::Parse,
            format!(
                "embedded data present but none parsed: {}",
                detail.join("; ")
            ),
            elapsed_ms(started),
        );
    }
    Envelope::ok(report, Trust::UntrustedPageContent, elapsed_ms(started))
}

/// 内联状态变量名（只认这些，避免把任意脚本当数据）。
const INLINE_STATE_VARS: [&str; 6] = [
    "window.__NUXT__",
    "window.__INITIAL_STATE__",
    "window.__PRELOADED_STATE__",
    "__APOLLO_STATE__",
    "window.__DATA__",
    "__INITIAL_STATE__",
];

fn push_entry(
    entries: &mut Vec<EmbeddedEntry>,
    options: &EmbeddedOptions,
    source: &str,
    name: Option<String>,
    payload: &str,
) {
    if entries.len() >= options.max_entries {
        return;
    }
    let bytes = payload.len();
    let truncated = bytes > options.max_entry_bytes;
    if truncated {
        entries.push(EmbeddedEntry {
            source: source.to_owned(),
            name,
            bytes,
            truncated: true,
            value: None,
            parse_error: None,
        });
        return;
    }
    let trimmed = payload.trim();
    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => entries.push(EmbeddedEntry {
            source: source.to_owned(),
            name,
            bytes,
            truncated: false,
            value: Some(value),
            parse_error: None,
        }),
        Err(error) => entries.push(EmbeddedEntry {
            source: source.to_owned(),
            name,
            bytes,
            truncated: false,
            value: None,
            parse_error: Some(if trimmed.is_empty() {
                "empty payload".to_owned()
            } else {
                format!("invalid JSON: {error}")
            }),
        }),
    }
}

fn conclusion(scanned: usize, entries: &[EmbeddedEntry], parsed: usize, failed: usize) -> String {
    if entries.is_empty() {
        return format!(
            "scanned {scanned} script tag(s); no embedded structured data found (this is a valid empty result, not a parse failure)"
        );
    }
    let truncated = entries.iter().filter(|e| e.truncated).count();
    format!(
        "scanned {scanned} script tag(s); found {} embedded payload(s): {parsed} parsed, {failed} failed, {truncated} truncated",
        entries.len()
    )
}

// ---------- 扫描器（线性、深度受限、无递归） ----------

/// ASCII 大小写不敏感查找（只对 ASCII 折叠，中文等字节按原样比较）。
fn find_ci(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    let hay = haystack.as_bytes();
    let pat: Vec<u8> = needle.bytes().map(|b| b.to_ascii_lowercase()).collect();
    let mut i = from;
    while i + pat.len() <= hay.len() {
        if hay[i..i + pat.len()]
            .iter()
            .zip(pat.iter())
            .all(|(a, b)| a.to_ascii_lowercase() == *b)
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 逐个取出 `<script …>…</script>`（返回：起始位置、开标签属性原文、脚本体）。
fn script_blocks(html: &str) -> Vec<(usize, &str, &str)> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(open) = find_ci(html, "<script", cursor) {
        let after = open + "<script".len();
        // 必须是标签：后面跟空白、`>` 或 `/`
        let boundary = html[after..].chars().next();
        if !matches!(boundary, Some(c) if c.is_whitespace() || c == '>' || c == '/') {
            cursor = after;
            continue;
        }
        let Some(gt_rel) = html[after..].find('>') else {
            break;
        };
        let gt = after + gt_rel;
        let attrs = &html[after..gt];
        let body_start = gt + 1;
        let Some(close) = find_ci(html, "</script", body_start) else {
            break;
        };
        out.push((open, attrs, &html[body_start..close]));
        cursor = close + "</script".len();
    }
    out
}

/// 判断 `script` 的类型：返回（来源名, type 值）。
fn classify_script(attrs: &str) -> (Option<&'static str>, Option<String>) {
    let Some(raw_type) = attr_value(attrs, "type") else {
        return (None, None);
    };
    let head = raw_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let kind = match head.as_str() {
        "application/ld+json" => Some("ld+json"),
        "application/json" | "text/json" => Some("script-json"),
        _ => None,
    };
    (kind, Some(raw_type))
}

/// 取属性值（支持双引号 / 单引号 / 裸值）。
fn attr_value(attrs: &str, name: &str) -> Option<String> {
    let mut cursor = 0usize;
    while let Some(at) = find_ci(attrs, name, cursor) {
        let before_ok = at == 0
            || matches!(
                attrs.as_bytes()[at - 1],
                b' ' | b'\t' | b'\n' | b'\r' | b'/'
            );
        let mut j = at + name.len();
        let bytes = attrs.as_bytes();
        while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
            j += 1;
        }
        if before_ok && j < bytes.len() && bytes[j] == b'=' {
            j += 1;
            while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
                j += 1;
            }
            if j >= bytes.len() {
                return None;
            }
            let quote = bytes[j];
            if quote == b'"' || quote == b'\'' {
                let from = j + 1;
                let rest = &attrs[from..];
                let end = rest.find(quote as char)?;
                return Some(rest[..end].to_owned());
            }
            let rest = &attrs[j..];
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '/')
                .unwrap_or(rest.len());
            return Some(rest[..end].to_owned());
        }
        cursor = at + name.len();
    }
    None
}

/// 取出某个属性在全文档里的所有值（用于 `data-page`）。
fn attr_values(html: &str, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(at) = find_ci(html, name, cursor) {
        // 从该位置起找"名字 + = 值"；attr_value 会检查前置边界与 `=`，因此不会误吞普通文本
        let window = &html[at..];
        if let Some(value) = attr_value(window, name) {
            out.push(value);
        }
        cursor = at + name.len();
        if out.len() >= MAX_EMBEDDED_ENTRIES {
            break;
        }
    }
    out
}

/// 找出 `var` 的赋值字面量（只接受对象/数组字面量）。
fn inline_state_literals(html: &str, var: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(at) = find_ci(html, var, cursor) {
        let mut j = at + var.len();
        let bytes = html.as_bytes();
        while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'=' {
            j += 1;
            while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
                j += 1;
            }
            if let Some(end) = scan_json_literal(html, j, MAX_LITERAL_DEPTH) {
                out.push((at, html[j..end].to_owned()));
            }
        }
        cursor = at + var.len();
    }
    out
}

/// 从 `start` 起扫描一个 `{…}` / `[…]` 字面量，返回结束位置（右括号之后）。
///
/// 只处理 JSON 结构：字符串（含转义）、括号配对、深度上限。
/// 遇到 `(` 或 `function` 开头的 IIFE 直接返回 None —— 那**不是** JSON 字面量，
/// 由调用方如实记为解析失败，而不是硬猜。
pub fn scan_json_literal(text: &str, start: usize, max_depth: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if start >= bytes.len() {
        return None;
    }
    let open = bytes[start];
    if open != b'{' && open != b'[' {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > max_depth {
                        return None;
                    }
                }
                b'}' | b']' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// 最小 HTML 实体反转义（`data-page` 里的 JSON 常被转义）。
pub fn unescape_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let replacement = [
            ("&quot;", "\""),
            ("&#34;", "\""),
            ("&apos;", "'"),
            ("&#39;", "'"),
            ("&lt;", "<"),
            ("&gt;", ">"),
            ("&amp;", "&"),
        ]
        .into_iter()
        .find(|(entity, _)| tail.starts_with(entity));
        match replacement {
            Some((entity, value)) => {
                out.push_str(value);
                rest = &tail[entity.len()..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_scan_handles_strings_and_escapes() {
        let text = r#"x = {"a":"}{","b":[1,2]};"#;
        let start = text.find('{').unwrap();
        let end = scan_json_literal(text, start, MAX_LITERAL_DEPTH).unwrap();
        assert_eq!(&text[start..end], r#"{"a":"}{","b":[1,2]}"#);
    }

    #[test]
    fn literal_scan_rejects_iife() {
        let text = "window.__NUXT__=(function(){return {}})();";
        let start = text.find('(').unwrap();
        assert!(scan_json_literal(text, start, MAX_LITERAL_DEPTH).is_none());
    }

    #[test]
    fn literal_scan_respects_depth_limit() {
        let text = "[[[[[[]]]]]]";
        assert!(scan_json_literal(text, 0, 3).is_none());
        assert!(scan_json_literal(text, 0, 8).is_some());
    }

    #[test]
    fn entities_are_unescaped() {
        assert_eq!(unescape_entities("&quot;a&quot;&amp;b"), "\"a\"&b");
    }
}
