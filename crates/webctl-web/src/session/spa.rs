//! SPA / JS 依赖页面的降级判定（PLAN v0.4 §2「会话」域，PR 04；正式归属 PR 09）。
//!
//! 诚实边界：v0.4 没有 JS 引擎。判定为 SPA 时必须显式 DEGRADED
//! （code + meta.degraded_from 成对），不得把空壳当成功、不得编造内容。
//! 判定刻意保守：**只要页面有实质可见正文就不降级**，避免把“脚本多的静态页”
//! 误判为 SPA（DEGRADE-06）。真正的完整降级语义在 PR 09 扩展。

use serde::{Deserialize, Serialize};

use super::html::{self, Node};

/// 判定为 SPA/JS 依赖页面的证据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaSignal {
    /// 人类可读的判定原因（必须说明依据）。
    pub reason: String,
    /// 逐条证据（空容器、框架指纹、脚本密度、noscript）。
    pub evidence: Vec<String>,
}

impl SpaSignal {
    /// 对应 Envelope 的 degraded_from 标签。
    pub const DEGRADED_FROM: &'static str = "js-render";
}

/// 空壳页面的可见文本阈值（含空白也算少）。
const SHELL_TEXT_LIMIT: usize = 80;

/// 探查页面是否依赖 JS 渲染；返回证据，未命中返回 None。
pub fn detect(html: &str) -> Option<SpaSignal> {
    let nodes = html::tokenize(html);
    let visible = visible_text_len(&nodes);
    let (script_count, script_bytes) = html::script_stats(html);
    let mut evidence: Vec<String> = Vec::new();
    let mut reasons: Vec<&str> = Vec::new();

    // 主信号必须足够强：空框架容器，或只有 noscript 的空壳。
    // 单凭框架属性不下结论（部分预渲染页面带 ng-app/data-reactroot 仍有正文）。
    if let Some(container) = empty_framework_container(&nodes) {
        evidence.push(format!("empty container {container}"));
        reasons.push("empty container");
    }
    if has_noscript(&nodes) && visible < SHELL_TEXT_LIMIT * 2 {
        evidence.push("noscript-only shell".to_owned());
        reasons.push("noscript");
    }
    if reasons.is_empty() {
        return None;
    }
    // 实质正文优先：有内容就不降级（避免误判，DEGRADE-06）。
    if visible >= SHELL_TEXT_LIMIT {
        return None;
    }
    if let Some(attr) = framework_attribute(&nodes) {
        evidence.push(format!("framework marker {attr}"));
        reasons.push("framework fingerprint");
    }
    if script_count > 0 {
        evidence.push(format!(
            "script density: {script_count} tags / {script_bytes} bytes, visible text {visible}"
        ));
    }
    let htmlish = html.len().max(1);
    if script_bytes * 100 / htmlish < 5 {
        evidence.push(format!("script bytes ratio low ({script_bytes}/{htmlish})"));
    }
    Some(SpaSignal {
        reason: format!(
            "page looks JS-rendered ({}); no browser engine in v0.4, so content is not rendered",
            reasons.join(", ")
        ),
        evidence,
    })
}

/// 返回空的框架根容器描述，如 div#root。
fn empty_framework_container(nodes: &[Node]) -> Option<String> {
    const ROOT_IDS: [&str; 4] = ["root", "app", "__next", "application"];
    for (index, node) in nodes.iter().enumerate() {
        let Node::Tag(tag) = node else { continue };
        if tag.closing || tag.name != "div" {
            continue;
        }
        let id = tag.attr("id").unwrap_or_default().to_ascii_lowercase();
        let class = tag.attr("class").unwrap_or_default().to_ascii_lowercase();
        let marker = if ROOT_IDS.contains(&id.as_str()) {
            Some(format!("div#{id}"))
        } else if class
            .split_whitespace()
            .any(|part| ROOT_IDS.contains(&part))
        {
            Some(format!("div.{class}"))
        } else {
            None
        };
        let Some(marker) = marker else { continue };
        if container_is_empty(nodes, index) {
            return Some(marker);
        }
    }
    None
}

fn container_is_empty(nodes: &[Node], start: usize) -> bool {
    for node in &nodes[start + 1..] {
        match node {
            Node::Tag(tag) if tag.closing && tag.name == "div" => return true,
            Node::Tag(_) => return false,
            Node::Text(text) if text.trim().is_empty() => continue,
            Node::Text(_) => return false,
        }
    }
    false
}

/// 框架指纹属性（React/Vue/Angular）。
fn framework_attribute(nodes: &[Node]) -> Option<String> {
    const MARKERS: [&str; 6] = [
        "ng-app",
        "ng-version",
        "data-reactroot",
        "data-v-app",
        "data-server-rendered",
        "v-cloak",
    ];
    for node in nodes {
        let Node::Tag(tag) = node else { continue };
        if tag.closing {
            continue;
        }
        for marker in MARKERS {
            if tag.has_attr(marker) {
                return Some(format!("{} on <{}>", marker, tag.name));
            }
        }
    }
    None
}

fn has_noscript(nodes: &[Node]) -> bool {
    nodes
        .iter()
        .any(|node| matches!(node, Node::Tag(tag) if !tag.closing && tag.name == "noscript"))
}

/// 不含 noscript 的可见文本长度。
fn visible_text_len(nodes: &[Node]) -> usize {
    let mut total = 0usize;
    let mut noscript_depth = 0usize;
    for node in nodes {
        match node {
            Node::Tag(tag) if tag.name == "noscript" => {
                if tag.closing {
                    noscript_depth = noscript_depth.saturating_sub(1);
                } else {
                    noscript_depth += 1;
                }
            }
            Node::Text(text) if noscript_depth == 0 => {
                total += text.chars().filter(|ch| !ch.is_whitespace()).count();
            }
            _ => {}
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_root_with_scripts_is_spa() {
        let html = r#"<html><body><div id="root"></div>
            <script src="/a.js"></script><script>window.x=1;</script>
            <script>window.y=2;</script></body></html>"#;
        let signal = detect(html).expect("spa");
        assert!(signal.reason.contains("empty container"));
        assert!(!signal.evidence.is_empty());
        assert_eq!(SpaSignal::DEGRADED_FROM, "js-render");
    }

    #[test]
    fn static_page_with_many_scripts_is_not_spa() {
        let long = "真实可见正文 ".repeat(30);
        let html = format!(
            r#"<html><body><article>{long}</article>
            <script src="/a.js"></script><script src="/b.js"></script>
            <script src="/c.js"></script><script>console.log(1);</script></body></html>"#
        );
        assert!(detect(&html).is_none(), "有实质正文的静态页不得降级");
    }

    #[test]
    fn noscript_only_shell_is_spa() {
        let html = r#"<html><body><noscript>请启用 JavaScript</noscript>
            <div id="app"></div><script src="/x.js"></script></body></html>"#;
        assert!(detect(html).is_some());
    }

    #[test]
    fn rendered_spa_marker_with_content_is_not_spa() {
        let html = r#"<html ng-app="demo"><body><div class="app">已经渲染好的正文内容</div></body></html>"#;
        assert!(
            detect(html).is_none(),
            "有内容的框架标记页不下结论（留给 PR 09）"
        );
    }
}
