//! JS 依赖页面的诚实判定（DEGRADE-01…06、09、10；SESS-10）。
//!
//! v0.4 没有 JS 引擎：页面若依赖客户端渲染，拿到的只是**预渲染 HTML**。
//! 本模块负责把这种页面识别出来，并给出**可解释**的依据，交给 super::guard
//! 写进结果信封。它只做判定与诊断，**绝不**补全、渲染或编造任何内容（DEGRADE-10）。
//!
//! # 判据（DEGRADE-03）
//!
//! 判定同时读取四类信号，并把它们逐条写进诊断：
//! 1. **脚本密度**：script 标签数与脚本正文字节数（含内联数据）；
//! 2. **空容器**：main / article / #root / #app 等主内容容器为空；
//! 3. **noscript 占比**：仅 noscript 里有文本、其余可见正文极少；
//! 4. **框架指纹**：ng-app / data-reactroot / __NEXT_DATA__ 等。
//!
//! # 不误判（DEGRADE-06）
//!
//! 只要页面有**足够的服务端可见正文**（见 STATIC_TEXT_MIN），无论脚本多少都判
//! Assessment::Rendered。脚本密度只作佐证，从不单独构成降级理由；图片/媒体等
//! 内容元素存在时也不把「无文本」当成空壳。判定刻意保守：宁可放过，不误伤静态页。

use serde::{Deserialize, Serialize};

use crate::session::html::{self, Node, Tag};

/// 写进 meta.degraded_from 的来源标签。
pub const DEGRADED_FROM: &str = "js-render";

/// 可见文本达到该字符数即认为服务端渲染充分，绝不降级（DEGRADE-06 的护栏）。
const STATIC_TEXT_MIN: usize = 120;
/// 可见文本低于该值才可能是空壳。
const SHELL_TEXT_MAX: usize = 40;
/// 「脚本多」的最小 script 标签数。
const SCRIPT_TAG_MIN: usize = 2;
/// 「脚本多」的最小脚本正文字节数。
const SCRIPT_BYTES_MIN: usize = 256;

/// 常见 SPA 挂载点 id。
const ROOT_IDS: [&str; 7] = [
    "root",
    "app",
    "__next",
    "__nuxt",
    "content",
    "main",
    "application",
];
/// 常见 SPA 挂载点 class（刻意不含通用的 container）。
const ROOT_CLASSES: [&str; 5] = ["root", "app", "content", "main", "application"];
/// 框架指纹属性（React / Vue / Angular / Svelte 等）。
const FRAMEWORK_ATTRS: [&str; 6] = [
    "ng-app",
    "ng-version",
    "data-reactroot",
    "data-v-app",
    "data-server-rendered",
    "v-cloak",
];
/// 框架指纹脚本标记（对原始 HTML 做小写子串匹配）。
const FRAMEWORK_SCRIPT_MARKERS: [&str; 3] = [
    "__next_data__",
    "window.__nuxt__",
    "window.__initial_state__",
];
/// 图片/媒体类内容元素：存在时即使没有文本也不算「空壳」。
const CONTENT_ELEMENTS: [&str; 10] = [
    "img", "picture", "video", "audio", "svg", "canvas", "iframe", "object", "embed", "figure",
];

/// 需要 JS 时的严重程度，决定空结果用哪个降级码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// 有占位容器或 noscript 回退等部分内容：结果不完整。
    Partial,
    /// 连占位/回退内容都没有：什么也拿不到。
    Total,
}

/// 一次判定给出的可解释诊断（DEGRADE-03）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnosis {
    /// 降级来源标签，恒为 DEGRADED_FROM。
    pub degraded_from: String,
    /// 汇总说明（写进 error），包含全部判定依据。
    pub reason: String,
    /// 逐条依据，便于机械核对。
    pub evidence: Vec<String>,
}

impl Diagnosis {
    /// DEGRADE-07：显式强制开关下的诊断——跳过启发式判定，原样返回响应体。
    pub fn forced() -> Self {
        Self {
            degraded_from: DEGRADED_FROM.to_owned(),
            reason: format!(
                "JS rendering skipped by explicit switch; the original response body is returned \
                 unchanged and nothing was rendered or synthesized ({DEGRADED_FROM})"
            ),
            evidence: vec!["forced: js-render switch is on (DEGRADE-07)".to_owned()],
        }
    }
}

/// 判定结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assessment {
    /// 服务端已渲染足够内容：不降级（静态脚本多页也走这里，DEGRADE-06）。
    Rendered,
    /// 页面依赖 JS；附带严重程度与依据。
    NeedsJs {
        /// 严重程度。
        severity: Severity,
        /// 可解释诊断。
        diagnosis: Diagnosis,
    },
}

/// 判定用到的量化信号（诊断与测试都读它）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Signals {
    /// 原始 HTML 字节数。
    pub html_bytes: usize,
    /// noscript 之外的可见文本字符数（不含空白）。
    pub visible_text: usize,
    /// noscript 内的文本字符数（不含空白）。
    pub noscript_text: usize,
    /// script 标签数。
    pub script_tags: usize,
    /// 脚本正文字节数（内联数据算在内）。
    pub script_bytes: usize,
    /// 是否存在内容容器（main / article / #root 等）。
    pub has_container: bool,
    /// 是否存在**空的**内容容器。
    pub empty_container: bool,
    /// 是否命中框架指纹。
    pub framework: bool,
    /// 是否存在图片/媒体类内容元素。
    pub has_content_elements: bool,
}

/// 探查页面是否依赖 JS（判定入口）。
pub fn assess(html: &str) -> Assessment {
    let nodes = html::tokenize(html);
    let signals = collect(html, &nodes);
    diagnose(&signals)
}

/// 只算信号、不下结论（供测试与审计）。
pub fn signals(html: &str) -> Signals {
    let nodes = html::tokenize(html);
    collect(html, &nodes)
}

fn diagnose(signals: &Signals) -> Assessment {
    // DEGRADE-06：静态正文足够就绝不降级，脚本再多也不影响。
    if signals.visible_text >= STATIC_TEXT_MIN {
        return Assessment::Rendered;
    }
    let shell_text = signals.visible_text <= SHELL_TEXT_MAX;
    let script_dense =
        signals.script_tags >= SCRIPT_TAG_MIN || signals.script_bytes >= SCRIPT_BYTES_MIN;
    let mut strong = false;
    let mut evidence: Vec<String> = Vec::new();

    if signals.empty_container && (signals.script_tags > 0 || signals.framework) {
        strong = true;
        evidence.push("empty container (main/article/#root/#app plus scripts)".to_owned());
    }
    let noscript_only = signals.noscript_text > 0 && shell_text;
    if noscript_only {
        strong = true;
        evidence.push(format!(
            "noscript fallback: {} chars inside noscript vs {} chars visible outside",
            signals.noscript_text, signals.visible_text
        ));
    }
    if signals.framework && shell_text {
        strong = true;
        evidence.push("framework fingerprint present".to_owned());
    }
    if shell_text && !signals.has_content_elements && script_dense {
        strong = true;
        evidence.push(format!(
            "no static content outside scripts: visible text {} chars, {} script tag(s), {} bytes",
            signals.visible_text, signals.script_tags, signals.script_bytes
        ));
    }
    if !strong {
        return Assessment::Rendered;
    }

    // 诊断始终补齐四类量化依据，便于 DEGRADE-03 复核。
    if signals.script_tags > 0 {
        let ratio = signals.script_bytes * 100 / signals.html_bytes.max(1);
        evidence.push(format!(
            "script density: {} tag(s) / {} bytes ({ratio}% of {} bytes)",
            signals.script_tags, signals.script_bytes, signals.html_bytes
        ));
    }
    let textual = signals.noscript_text + signals.visible_text;
    let noscript_ratio = if textual == 0 {
        0
    } else {
        signals.noscript_text * 100 / textual
    };
    evidence.push(format!(
        "visible text: {} chars; noscript share: {noscript_ratio}% of {textual} textual chars",
        signals.visible_text
    ));

    let severity = if signals.visible_text == 0
        && signals.noscript_text == 0
        && !signals.empty_container
        && !signals.has_content_elements
    {
        Severity::Total
    } else {
        Severity::Partial
    };
    let reason = format!(
        "page requires client-side JavaScript, which the v0.4 builtin engine cannot execute; \
         the pre-render HTML was returned unchanged: {}",
        evidence.join("; ")
    );
    Assessment::NeedsJs {
        severity,
        diagnosis: Diagnosis {
            degraded_from: DEGRADED_FROM.to_owned(),
            reason,
            evidence,
        },
    }
}

fn collect(html: &str, nodes: &[Node]) -> Signals {
    let (script_tags, script_bytes) = html::script_stats(html);
    let empty_container = find_empty_container(nodes);
    let mut signals = Signals {
        html_bytes: html.len(),
        script_tags,
        script_bytes,
        empty_container: empty_container.is_some(),
        ..Signals::default()
    };
    let mut noscript_depth = 0usize;
    let mut framework: Option<&'static str> = None;
    for node in nodes {
        match node {
            Node::Text(text) => {
                let count = text.chars().filter(|ch| !ch.is_whitespace()).count();
                if noscript_depth > 0 {
                    signals.noscript_text += count;
                } else {
                    signals.visible_text += count;
                }
            }
            Node::Tag(tag) => {
                if tag.name == "noscript" {
                    if tag.closing {
                        noscript_depth = noscript_depth.saturating_sub(1);
                    } else {
                        noscript_depth += 1;
                    }
                    continue;
                }
                if tag.closing {
                    continue;
                }
                if container_label(tag).is_some() {
                    signals.has_container = true;
                }
                if CONTENT_ELEMENTS.contains(&tag.name.as_str()) {
                    signals.has_content_elements = true;
                }
                if framework.is_none() {
                    for marker in FRAMEWORK_ATTRS {
                        if tag.has_attr(marker) {
                            framework = Some(marker);
                            break;
                        }
                    }
                }
            }
        }
    }
    if framework.is_none() {
        let lower = html.to_ascii_lowercase();
        for marker in FRAMEWORK_SCRIPT_MARKERS {
            if lower.contains(marker) {
                framework = Some(marker);
                break;
            }
        }
    }
    signals.framework = framework.is_some();
    signals
}

/// 找到第一个**为空**的内容容器，返回可读标签（如 div#root）。
fn find_empty_container(nodes: &[Node]) -> Option<String> {
    for (index, node) in nodes.iter().enumerate() {
        let Node::Tag(tag) = node else { continue };
        if tag.closing {
            continue;
        }
        let Some(label) = container_label(tag) else {
            continue;
        };
        if container_is_empty(nodes, index, &tag.name) {
            return Some(label);
        }
    }
    None
}

/// 容器从开始标签到匹配结束标签之间是否没有任何内容。
///
/// 保守取向：出现**任何**非 script/style 子元素即视为有内容（宁可漏判，不误判）。
fn container_is_empty(nodes: &[Node], open: usize, name: &str) -> bool {
    let mut depth = 0usize;
    for node in &nodes[open + 1..] {
        match node {
            Node::Tag(tag) if tag.name == name => {
                if tag.closing {
                    if depth == 0 {
                        return true;
                    }
                    depth -= 1;
                } else {
                    depth += 1;
                }
            }
            Node::Tag(tag) if !tag.closing && !matches!(tag.name.as_str(), "script" | "style") => {
                return false;
            }
            Node::Text(text) if !text.trim().is_empty() => return false,
            _ => {}
        }
    }
    // 没有闭合标签：保守视为有内容。
    false
}

/// 元素是否是内容容器；是则返回可读标签。
fn container_label(tag: &Tag) -> Option<String> {
    if matches!(tag.name.as_str(), "main" | "article") {
        return Some(tag.name.clone());
    }
    if let Some(id) = tag.attr("id") {
        let id = id.trim().to_ascii_lowercase();
        if ROOT_IDS.contains(&id.as_str()) {
            return Some(format!("{}#{}", tag.name, id));
        }
    }
    if let Some(class) = tag.attr("class") {
        for token in class.split_whitespace() {
            let token = token.to_ascii_lowercase();
            if ROOT_CLASSES.contains(&token.as_str()) {
                return Some(format!("{}.{}", tag.name, token));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_text_short_circuits_even_with_scripts() {
        let text = "真实可见正文内容".repeat(20);
        let html = format!(
            "<html><body><div class=\"article\">{text}</div>\
             <script src=\"/a.js\"></script><script>window.x=1;</script></body></html>"
        );
        assert_eq!(assess(&html), Assessment::Rendered);
    }

    #[test]
    fn empty_root_with_scripts_is_partial() {
        let html = r#"<html><body><div id="root"></div>
                    <script src="/a.js"></script><script>boot();</script></body></html>"#;
        let Assessment::NeedsJs {
            severity,
            diagnosis,
        } = assess(html)
        else {
            panic!("expected needs-js");
        };
        assert_eq!(severity, Severity::Partial);
        assert!(diagnosis.reason.contains("empty container"));
    }

    #[test]
    fn blank_shell_without_placeholder_is_total() {
        let html = r#"<html><head></head><body>
                    <script src="/bundle.js"></script><script>boot();</script></body></html>"#;
        let Assessment::NeedsJs { severity, .. } = assess(html) else {
            panic!("expected needs-js");
        };
        assert_eq!(severity, Severity::Total);
    }

    #[test]
    fn image_only_static_page_is_not_shell() {
        let html = r#"<html><body><img src="/p.png" alt="photo">
                    <script src="/a.js"></script><script>window.x=1;</script></body></html>"#;
        assert_eq!(assess(html), Assessment::Rendered);
    }
}
