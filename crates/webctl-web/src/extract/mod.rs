//! 抽取引擎（PLAN v0.4 §3.5，PR 06）。
//!
//! v0.4 没有 live DOM，也没有 v0.3 的 aria ref。因此「引用」改为：
//! CSS 选择器 + **结构化路径**（tag / 同级索引 / 祖先链 / 可复现 CSS 路径）+
//! 文本上下文；表单交互用 (form_index, field_name) 稳定寻址。
//!
//! # 稳定性语义（EXTRACT-13，必须记住）
//!
//! **同一选择器跨请求不保证命中同一元素。** 每次抓取/解析都是全新的文档：
//! 服务端可能返回不同内容（A/B、时间戳、登录态、分页漂移），选择器也可能因
//! 客户端渲染而落空。本模块只保证**单次调用内**命中按文档序稳定，不提供、
//! 也不假装提供跨请求的元素身份。需要定位元素时，用返回的 ElementPath
//! （css / ancestors）在**同一次解析结果**内寻址，或改用
//! (form_index, field_name) 这类由页面结构定义的稳定坐标。
//!
//! # 五种结果严格区分（禁止静默降级）
//!
//! | 情形 | ok | code | meta.degraded_from | data |
//! |---|---|---|---|---|
//! | 零命中（合法空结果） | true | OK | null | 空数组 + 可读 conclusion |
//! | 非法选择器 / 正则 | false | PARSE | null | null |
//! | 非 HTML 内容（pdf/binary 等） | true | UNSUPPORTED | 非空 | 说明性结果 |
//! | 恶意选择器 / 超长正则被拒 | false | BLOCKED | null | null |
//! | 预算 / 命中上限截断 | true | LIMIT | 非空 | 截断后的结果 + 标记 |
//! | 超出墙钟预算 | false | TIMEOUT | null | null |
//!
//! 五者码互不相同：空结果不会被当成失败，解析失败不会返回空数组冒充成功，
//! 截断一定同时带 code=LIMIT 与非空 meta.degraded_from。

use std::time::Instant;

use serde::{Deserialize, Serialize};
use webctl_core::{Envelope, ErrorCode, Trust};

use crate::parse::elapsed_ms;

pub mod context;
pub mod links;
pub mod path;
pub mod search;
pub mod structured;

pub use context::{head_chars, tail_chars};

/// `extract` 是 `select_matches` 的别名，方便 CLI/MCP 以工具名 `extract` 暴露。
pub use self::select_matches as extract;
pub use links::{FilteredLinks, LinkFilter, links};
pub use search::{SearchMatch, SearchOptions, SearchResult, search};
pub use structured::{extract_body, form_by_index, forms, tables};

/// 抽取引擎的默认稳定性声明，直接暴露给调用方写进输出。
pub const STABILITY_NOTE: &str = "selector matches are not stable across requests: every parse is a fresh document; \
     results are ordered by document order within a single call only";

/// extract 一次调用的选项。全部字段都有保守默认值（见 Default）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractOptions {
    /// 命中文本上下文的前后字符数。
    pub context_chars: usize,
    /// 最多返回多少条命中；超出即截断并标 LIMIT。
    pub max_results: usize,
    /// text + before + after 的总字符预算；超出即截断并标 LIMIT。
    pub char_budget: usize,
    /// 单条命中 html 字段的字符上限（逐条截断，避免大页面塞爆输出）。
    pub max_html_chars: usize,
    /// 选择器字节数上限；超过直接 BLOCKED（拒绝评估不受信选择器）。
    pub max_selector_len: usize,
    /// 选择器复杂度上限（组合子 / 伪类 / 属性选择器等标记计数）。
    pub max_selector_complexity: usize,
    /// 墙钟预算（毫秒）；超时返回 TIMEOUT。0 表示不分配任何时间。
    pub max_elapsed_ms: u64,
    /// 扫描命中的硬上限，防「选择器命中一切」时迭代无界。
    pub max_scan_matches: usize,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            context_chars: 40,
            max_results: 50,
            char_budget: 8_000,
            max_html_chars: 2_000,
            max_selector_len: 512,
            max_selector_complexity: 64,
            max_elapsed_ms: 5_000,
            max_scan_matches: 10_000,
        }
    }
}

/// 结构化路径中的一个祖先段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathSegment {
    /// 标签名（小写）。
    pub tag: String,
    /// 该段在同级**元素**中的 0 起索引。
    pub sibling_index: usize,
}

/// 一条命中的结构化路径（PARSE-12 / EXTRACT-01）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementPath {
    /// 命中元素标签名（小写）。
    pub tag: String,
    /// 命中元素在同级元素中的 0 起索引。
    pub sibling_index: usize,
    /// 祖先链，从根（通常是 html）到父元素。
    pub ancestors: Vec<PathSegment>,
    /// 祖先层数（ancestors.len()）。
    pub depth: usize,
    /// 可复现的 CSS 路径，例如 html:nth-of-type(1) > body:nth-of-type(1) > p:nth-of-type(2)。
    pub css: String,
}

/// 一条选择器命中。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractMatch {
    /// 命中序号（0 起，按文档序）。
    pub index: usize,
    /// 标签名（小写）。
    pub tag: String,
    /// 结构化路径。
    pub path: ElementPath,
    /// 元素后代文本，折叠空白。
    pub text: String,
    /// text 是否因字符预算被截断。
    pub text_truncated: bool,
    /// 元素 outer HTML（逐条截断到 max_html_chars，可审计的 UTF-8 文本）。
    pub html: String,
    /// html 是否被截断。
    pub html_truncated: bool,
    /// 命中前的文本上下文（折叠空白，至多 context_chars 字符）。
    pub before: String,
    /// 命中后的文本上下文（折叠空白，至多 context_chars 字符）。
    pub after: String,
}

/// extract 的结果集。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractResult {
    /// 选择器原文。
    pub selector: String,
    /// 命中（按文档序）。
    pub matches: Vec<ExtractMatch>,
    /// 扫描到的命中数（受 max_results / max_scan_matches 限制，可能小于真实总数）。
    pub observed_matches: usize,
    /// 是否因上限提前停止扫描（后面还有命中）。
    pub more_matches: bool,
    /// 是否发生了截断。
    pub truncated: bool,
    /// 截断原因，可能为 max-results / char-budget / scan-cap。
    pub truncated_reasons: Vec<String>,
    /// 可读结论（零命中时解释可能原因）。
    pub conclusion: String,
    /// 稳定性声明，恒为 STABILITY_NOTE。
    pub stability: String,
}

impl ExtractResult {
    fn empty(selector: &str) -> Self {
        Self {
            selector: selector.to_owned(),
            matches: Vec::new(),
            observed_matches: 0,
            more_matches: false,
            truncated: false,
            truncated_reasons: Vec::new(),
            conclusion: String::new(),
            stability: STABILITY_NOTE.to_owned(),
        }
    }

    /// 为「非 HTML 内容」构造说明性结果（UNSUPPORTED 降级用）。
    pub(crate) fn unsupported(selector: &str, reason: String) -> Self {
        let mut result = Self::empty(selector);
        result.conclusion = reason;
        result
    }
}

/// 用 CSS 选择器抽取元素，返回**保序**的结构化命中（PARSE-12 / EXTRACT-01…07、14、16、17）。
///
/// 语义边界：
/// - 合法选择器 + 零命中 → ok=true + 空数组 + 可读 conclusion；
/// - 空/非法选择器 → ok=false + code=PARSE（不返回空数组冒充成功，不 panic）；
/// - 超长/超复杂选择器 → ok=false + code=BLOCKED（拒绝评估不受信选择器）；
/// - 命中数超 max_results、字符超 char_budget、扫描超 max_scan_matches
///   → ok=true + code=LIMIT + 非空 meta.degraded_from；
/// - 超过 max_elapsed_ms（含 parse_document 与逐元素扫描本身）→ ok=false + code=TIMEOUT。
///
/// 结果只含 UTF-8 文本字段，不夹带原始字节（EXTRACT-16）。
pub fn select_matches(
    html_text: &str,
    selector: &str,
    options: &ExtractOptions,
) -> Envelope<ExtractResult> {
    let started = Instant::now();
    if let Some((code, message)) = guard_selector(selector, options) {
        return Envelope::fail(code, message, elapsed_ms(started));
    }
    let css = match crate::parse::html::compile_selector(selector) {
        Ok(css) => css,
        Err(message) => return Envelope::fail(ErrorCode::Parse, message, elapsed_ms(started)),
    };
    let document = crate::parse::html::parse_document(html_text);
    // 解析耗时也必须计入墙钟预算：否则大页面在解析阶段就能轻松越过预算，
    // 而零命中时后面没有任何检查点（审阅者实测 1MB 零命中 → 4s + OK）。
    if let Some(envelope) = budget_timeout(started, options.max_elapsed_ms, "extract") {
        return envelope;
    }
    let index = context::DocumentText::build(&document);
    if let Some(envelope) = budget_timeout(started, options.max_elapsed_ms, "extract") {
        return envelope;
    }

    let mut result = ExtractResult::empty(selector);
    let mut used_chars = 0usize;
    let mut reasons: Vec<String> = Vec::new();
    let mut more = false;

    // 逐元素手动推进，而不是 document.select(&css)：后者只在「命中时」才把控制权
    // 交回循环体，零命中时整轮扫描发生在一次 next() 内部，预算检查永远轮不到
    // （这正是审阅者实测的 1MB 零命中 → 4s + OK）。这里在每次元素推进前检查预算，
    // 扫描本身因而受 max_elapsed_ms 约束；语义与 Select 一致（跳过无父节点者）。
    for node in document.tree.nodes() {
        let Some(element) = scraper::ElementRef::wrap(node) else {
            continue;
        };
        if element.parent().is_none() {
            continue;
        }
        if let Some(envelope) = budget_timeout(started, options.max_elapsed_ms, "extract") {
            return envelope;
        }
        if !css.matches(&element) {
            continue;
        }
        result.observed_matches += 1;
        if result.matches.len() >= options.max_results {
            more = true;
            reasons.push("max-results".to_owned());
            break;
        }
        if result.observed_matches > options.max_scan_matches {
            more = true;
            reasons.push("scan-cap".to_owned());
            break;
        }

        let (mut hit, cost) = build_match(result.matches.len(), &element, &index, options);
        let remaining = options.char_budget.saturating_sub(used_chars);
        if cost > remaining {
            reasons.push("char-budget".to_owned());
            more = true;
            if remaining > 0 && result.matches.is_empty() {
                // 至少保留一条（截断后的）命中，避免「有命中」被误读成「零命中」。
                hit.text = head_chars(&hit.text, remaining);
                hit.text_truncated = true;
                hit.before.clear();
                hit.after.clear();
                result.matches.push(hit);
            }
            break;
        }
        used_chars += cost;
        result.matches.push(hit);
    }

    // 扫描结束（自然结束或 break）后再兜底一次：最后一次推进可能刚好越界。
    if let Some(envelope) = budget_timeout(started, options.max_elapsed_ms, "extract") {
        return envelope;
    }

    if reasons.is_empty() {
        result.conclusion = if result.matches.is_empty() {
            zero_conclusion(selector)
        } else {
            format!(
                "selector {selector:?} matched {} element(s) in document order",
                result.matches.len()
            )
        };
        Envelope::ok(result, Trust::UntrustedPageContent, elapsed_ms(started))
    } else {
        result.truncated = true;
        result.more_matches = more;
        result.truncated_reasons = dedup_reasons(&reasons);
        result.conclusion = format!(
            "selector {selector:?} results truncated ({}) with {} element(s) returned",
            result.truncated_reasons.join(", "),
            result.matches.len()
        );
        let from = result.truncated_reasons.join(",");
        Envelope::degraded(result, from, ErrorCode::Limit, elapsed_ms(started))
    }
}

/// 墙钟预算兜底：自 started 起已用满 budget_ms 时返回 TIMEOUT 信封。
///
/// 调用点必须覆盖「零命中也能到达」的位置：解析 / 建索引之后、逐元素扫描的每次
/// 推进、以及扫描结束构造结果之前。budget_ms=0 表示不分配任何时间，任何耗时都超时。
fn budget_timeout<T>(started: Instant, budget_ms: u64, what: &str) -> Option<Envelope<T>> {
    if elapsed_ms(started) >= budget_ms {
        Some(Envelope::fail(
            ErrorCode::Timeout,
            format!("{what} exceeded wall-clock budget of {budget_ms} ms"),
            elapsed_ms(started),
        ))
    } else {
        None
    }
}

/// 零命中的可读结论：明确是「合法空结果」，并给出可能原因。
fn zero_conclusion(selector: &str) -> String {
    format!(
        "selector {selector:?} matched 0 elements; this is a valid empty result, not a parse \
         failure. The page may not contain the element, or the content may be produced \
         client-side (v0.4 has no JS engine). Note: the same selector is not guaranteed to \
         match the same element across requests."
    )
}

/// 选择器长度 / 复杂度守卫（EXTRACT-17）。返回拒绝码与可读原因。
fn guard_selector(css: &str, options: &ExtractOptions) -> Option<(ErrorCode, String)> {
    if css.trim().is_empty() {
        return None; // 交给 compile_selector 产出 PARSE。
    }
    if css.len() > options.max_selector_len {
        return Some((
            ErrorCode::Blocked,
            format!(
                "selector rejected: {} bytes exceeds cap {} (refusing to evaluate an untrusted oversized selector)",
                css.len(),
                options.max_selector_len
            ),
        ));
    }
    let complexity = selector_complexity(css);
    if complexity > options.max_selector_complexity {
        return Some((
            ErrorCode::Blocked,
            format!(
                "selector rejected: complexity {complexity} exceeds cap {} (refusing to evaluate an untrusted selector)",
                options.max_selector_complexity
            ),
        ));
    }
    None
}

/// 选择器复杂度：组合子 / 分组 / 伪类 / 属性选择器 / 函数调用的标记计数 + 1。
///
/// 这是**保守上界**，不是精确的语法分析：任何会显著增加 cssparser 回溯的构造
/// （大量 >、:、[、(）都会抬高计数，从而在编译前被拒绝。
fn selector_complexity(css: &str) -> usize {
    css.bytes()
        .filter(|byte| matches!(byte, b'>' | b'+' | b'~' | b',' | b':' | b'[' | b'('))
        .count()
        + 1
}

/// 按文档序构造一条命中，返回 (命中, 计入字符预算的成本)。
fn build_match(
    index: usize,
    element: &scraper::ElementRef<'_>,
    document_text: &context::DocumentText<'_>,
    options: &ExtractOptions,
) -> (ExtractMatch, usize) {
    let text = crate::parse::html::collapse_ws(&element.text().collect::<String>());
    let (before, after) = match document_text.range_of(element) {
        Some((start, end)) => (
            crate::parse::html::collapse_ws(&document_text.before(start, options.context_chars)),
            crate::parse::html::collapse_ws(&document_text.after(end, options.context_chars)),
        ),
        None => (String::new(), String::new()),
    };
    let raw_html = element.html();
    let html_truncated = raw_html.chars().count() > options.max_html_chars;
    let html = head_chars(&raw_html, options.max_html_chars);
    let cost = text.chars().count() + before.chars().count() + after.chars().count();
    (
        ExtractMatch {
            index,
            tag: element.value().name().to_ascii_lowercase(),
            path: path::element_path(element),
            text,
            text_truncated: false,
            html,
            html_truncated,
            before,
            after,
        },
        cost,
    )
}

fn dedup_reasons(reasons: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for reason in reasons {
        if !out.contains(reason) {
            out.push(reason.clone());
        }
    }
    out
}
