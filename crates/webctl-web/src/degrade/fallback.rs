//! 结构化回退提示（议题 #224，路线图 0.2.3 §3.1）。
//!
//! 当 fetch / extract 因页面依赖客户端渲染而降级时，本模块把"为什么适合/不适合
//! 回退到外部浏览器"整理成 [`FallbackHint`] 填入 `meta.actionable.fallback_hint`。
//!
//! **纯建议，零外部调用**：这里只读入 HTML、做静态字符串/标签扫描；不启动浏览器、
//! 不起子进程、不发请求。是否真的回退由调用方（agent/宿主）决定。
//!
//! 信号面板（任一强信号命中即 `playwright_suitable = true`）：
//! - 空的 `#root` / `#app` 挂载点；
//! - 解析后可见正文几乎为空；
//! - `script[src]` 里出现 `chunk`（打包产物）；
//! - hydration 标记（`data-reactroot`、`__NEXT_DATA__`、`window.__NUXT__`）。
//!
//! `js_engine_suitable`：依赖外链库（绝对 URL 或常见库名）时为 false——进程内
//! JS 引擎不会代取远端库，执行不了这类页面。

use webctl_core::FallbackHint;

use crate::session::html::{self, Node};

/// 可见正文低于该字符数视为"解析后几乎为空"。
const BODY_TEXT_MIN: usize = 40;

/// 只认这两个 SPA 挂载点 id（task #224 明确要求 #root / #app）。
const ROOT_IDS: [&str; 2] = ["root", "app"];

/// hydration 属性标记（标签属性，大小写不敏感）。
const HYDRATION_ATTRS: [&str; 2] = ["data-reactroot", "data-server-rendered"];

/// hydration 脚本标记（对原始 HTML 做小写子串匹配）。
const HYDRATION_MARKERS: [&str; 3] = ["__next_data__", "window.__nuxt__", "__nuxt__"];

/// 外链库特征（script[src] 小写匹配）。
const EXTERNAL_LIBRARY_MARKERS: [&str; 9] = [
    "jquery",
    "react",
    "vue",
    "angular",
    "bootstrap",
    "lodash",
    "cdn.",
    "unpkg",
    "jsdelivr",
];

/// 采集页面信号并生成回退提示；没有任何强信号时返回 None（纯静态页不出现该字段）。
///
/// `embedded_data_tried`：由调用方说明内嵌数据抽取是否已尝试且无果。当前
/// fetch/extract 不做自动抽取，因此 guard 传 false；字段保留给后续接线。
pub fn hint(html: &str, embedded_data_tried: bool) -> Option<FallbackHint> {
    let nodes = html::tokenize(html);
    let mut signals: Vec<String> = Vec::new();

    if let Some(container) = empty_root_container(&nodes) {
        signals.push(format!("{container} empty"));
    }
    if visible_text(&nodes) <= BODY_TEXT_MIN {
        signals.push("body text nearly empty".to_owned());
    }
    if has_chunk_script(&nodes) {
        signals.push("script[src*=chunk]".to_owned());
    }
    for marker in hydration_marker(html, &nodes) {
        signals.push(format!("hydration marker: {marker}"));
    }

    if signals.is_empty() {
        return None;
    }
    Some(FallbackHint {
        playwright_suitable: true,
        signals,
        embedded_data_tried,
        js_engine_suitable: !depends_on_external_library(&nodes),
    })
}

/// 不含 noscript 的可见文本字符数（忽略空白）。
fn visible_text(nodes: &[Node]) -> usize {
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

/// 找到空的 #root / #app 容器，返回可读标签（如 `div#root`）。
fn empty_root_container(nodes: &[Node]) -> Option<String> {
    for (index, node) in nodes.iter().enumerate() {
        let Node::Tag(tag) = node else { continue };
        if tag.closing {
            continue;
        }
        let id = tag
            .attr("id")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if !ROOT_IDS.contains(&id.as_str()) {
            continue;
        }
        if container_is_empty(nodes, index, &tag.name) {
            return Some(format!("{}#{}", tag.name, id));
        }
    }
    None
}

/// 容器从开始标签到匹配结束标签之间是否没有任何内容。
///
/// 保守取向：出现**任何**非 script/style 子元素或文本即视为有内容（宁可漏判，不误判）。
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
    false
}

/// script[src] 是否含 `chunk`（打包产物的稳定特征）。
fn has_chunk_script(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| {
        let Node::Tag(tag) = node else { return false };
        !tag.closing
            && tag.name == "script"
            && tag
                .attr("src")
                .is_some_and(|src| src.to_ascii_lowercase().contains("chunk"))
    })
}

/// 命中的 hydration 标记（属性优先，再扫脚本标记）；去重保序。
fn hydration_marker(html: &str, nodes: &[Node]) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut hits: Vec<String> = Vec::new();
    for node in nodes {
        let Node::Tag(tag) = node else { continue };
        if tag.closing {
            continue;
        }
        for attr in HYDRATION_ATTRS {
            if tag.has_attr(attr) {
                hits.push((*attr).to_owned());
            }
        }
    }
    for marker in HYDRATION_MARKERS {
        if lower.contains(marker) {
            hits.push((*marker).to_owned());
        }
    }
    hits.sort_unstable();
    hits.dedup();
    hits
}

/// 是否依赖外链库：绝对 URL 的 script，或 src 命中常见库名/CDN。
fn depends_on_external_library(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| {
        let Node::Tag(tag) = node else { return false };
        if tag.closing || tag.name != "script" {
            return false;
        }
        let Some(src) = tag.attr("src") else {
            return false;
        };
        let lower = src.trim().to_ascii_lowercase();
        lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("//")
            || EXTERNAL_LIBRARY_MARKERS
                .iter()
                .any(|marker| lower.contains(marker))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const JS_SHELL: &str = r#"<!DOCTYPE html>
<html><head><title>App</title>
<script src="/static/js/chunk-vendors.js"></script></head>
<body><div id="root"></div><script>window.__NUXT__ = {};</script></body></html>"#;

    const STATIC: &str = r#"<!DOCTYPE html>
<html><head><title>Docs</title></head><body><article>
<h1>Release notes</h1>
<p>This page is fully server rendered and contains enough visible text to rule out a JS shell.</p>
</article></body></html>"#;

    #[test]
    fn js_shell_hint_lists_signals() {
        let hint = hint(JS_SHELL, false).expect("hint");
        assert!(hint.playwright_suitable);
        assert!(hint.signals.iter().any(|signal| signal.contains("#root")));
        assert!(hint.signals.iter().any(|signal| signal.contains("chunk")));
        assert!(!hint.embedded_data_tried);
        assert!(hint.js_engine_suitable);
    }

    #[test]
    fn static_page_has_no_hint() {
        assert!(hint(STATIC, false).is_none());
    }

    #[test]
    fn external_library_blocks_builtin_engine() {
        let html = r#"<html><body><div id="app"></div>
            <script src="https://cdn.jsdelivr.net/npm/vue@3/dist/vue.global.prod.js"></script>
            </body></html>"#;
        let hint = hint(html, false).expect("hint");
        assert!(hint.playwright_suitable);
        assert!(!hint.js_engine_suitable);
    }

    #[test]
    fn filled_root_is_not_an_empty_container() {
        let html = r#"<html><body><div id="app"><p>already rendered</p></div>
            <script src="/static/js/chunk-main.js"></script></body></html>"#;
        let hint = hint(html, false).expect("chunk signal");
        assert!(!hint.signals.iter().any(|signal| signal.contains("#app")));
    }
}
