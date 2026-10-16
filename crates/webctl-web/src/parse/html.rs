//! HTML 解析共享工具：文档解析、选择器、噪声剥离、纯文本与 markdown 渲染。
//!
//! - 选择器一律用 cssparser/scraper 的 CSS 语法；编译失败返回可读错误字符串，
//!   由调用方映射为 code=PARSE。
//! - 噪声剥离与 markdown 渲染都走**显式栈**的迭代遍历，避免极深嵌套触发栈溢出。
//! - 纯文本视图交给 rust-html2text（PLAN §3.2 的 HTML→文本选型）。

use html2text::from_read;
use scraper::{ElementRef, Html, Node, Selector};
use url::Url;

/// 纯文本视图的排版宽度。取足够大的值以避免长句被硬换行破坏。
pub const PLAIN_TEXT_WIDTH: usize = 1_000_000;

/// 交给 Readability 主路径的最大元素嵌套深度。
///
/// dom_smoothie 在极深嵌套上呈超线性退化（实测 2000 层需要数分钟），因此超过
/// 该阈值的文档直接走整页回退（显式 DEGRADED），既避免超时也不栈溢出。
pub const MAX_READABILITY_DEPTH: usize = 128;

/// 迭代计算最大元素嵌套深度；一旦超过 MAX_READABILITY_DEPTH 立即返回。
pub fn max_element_depth(document: &Html) -> usize {
    let mut max = 0usize;
    let mut stack = vec![(document.root_element(), 0usize)];
    while let Some((element, depth)) = stack.pop() {
        let current = depth + 1;
        if current > max {
            max = current;
        }
        if max > MAX_READABILITY_DEPTH {
            return max;
        }
        for child in element.children() {
            if let Some(child_element) = ElementRef::wrap(child) {
                stack.push((child_element, current));
            }
        }
    }
    max
}

/// 噪声标签：整页回退与 markdown 渲染都跳过这些子树。
///
/// 注意：正文主路径（dom_smoothie）自带 Readability 清洗；这里是回退路径的
/// 保守名单，只用语义明确的容器，不做 class/id 启发式猜测。
pub const NOISE_TAGS: &[&str] = &[
    "script", "style", "noscript", "template", "nav", "footer", "header", "aside", "form",
    "iframe", "svg", "canvas", "object", "embed", "dialog",
];

/// void 元素（没有闭合标签、没有子节点）。
const VOID_TAGS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// 解析 HTML 文档（html5ever，容错解析：畸形标签不报错、不 panic）。
pub fn parse_document(source: &str) -> Html {
    Html::parse_document(source)
}

/// 编译 CSS 选择器；空选择器与非法选择器都返回可读错误。
pub fn compile_selector(css: &str) -> Result<Selector, String> {
    if css.trim().is_empty() {
        return Err("empty selector".to_owned());
    }
    Selector::parse(css).map_err(|error| format!("invalid selector {css:?}: {error:?}"))
}

/// 编译一个静态选择器（仅用于内部固定字面量）。
pub fn selector(css: &str) -> Option<Selector> {
    Selector::parse(css).ok()
}

/// 折叠空白：连续空白替换为单个空格并去掉首尾。
pub fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 噪声标签判定。
pub fn is_noise(tag: &str) -> bool {
    NOISE_TAGS.contains(&tag)
}

/// void 元素判定。
pub fn is_void(tag: &str) -> bool {
    VOID_TAGS.contains(&tag)
}

/// 文档标题（title 元素文本）；缺失时返回 None。
pub fn document_title(document: &Html) -> Option<String> {
    let css = selector("title")?;
    let element = document.select(&css).next()?;
    let title = collapse_ws(&element.text().collect::<String>());
    if title.is_empty() { None } else { Some(title) }
}

/// 文档的解析基准：先看文档 URL，再看第一个 base[href]（按文档 URL 展开）。
pub fn resolve_base(document: &Html, document_url: Option<&str>) -> Option<Url> {
    let doc_url = document_url.and_then(|url| Url::parse(url).ok());
    let base_href = selector("base[href]")
        .and_then(|css| document.select(&css).next())
        .and_then(|element| element.attr("href"))
        .map(str::to_owned);
    match (base_href, doc_url) {
        (Some(href), Some(base)) => base.join(&href).ok(),
        (Some(href), None) => Url::parse(&href).ok(),
        (None, base) => base,
    }
}

/// body 元素；文档没有显式 body 时回退到根元素。
pub fn body_element(document: &Html) -> ElementRef<'_> {
    selector("body")
        .and_then(|css| document.select(&css).next())
        .unwrap_or_else(|| document.root_element())
}

/// 渲染纯文本视图（rust-html2text）。
pub fn render_plain_text(source: &str) -> String {
    from_read(source.as_bytes(), PLAIN_TEXT_WIDTH).unwrap_or_default()
}

/// 渲染 markdown 视图（迭代遍历；标题/列表/链接/强调都有确定标记）。
pub fn render_markdown(source: &str) -> String {
    let document = Html::parse_fragment(source);
    let mut out = String::new();
    let mut lists: Vec<(bool, usize)> = Vec::new();
    let mut stack: Vec<MdStep<'_>> = vec![MdStep::Open(document.root_element())];
    while let Some(step) = stack.pop() {
        match step {
            MdStep::Open(element) => {
                let tag = element.value().name();
                if is_noise(tag) {
                    continue;
                }
                let mut href: Option<String> = None;
                match tag {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        ensure_block(&mut out);
                        let level = usize::from(tag.as_bytes()[1] - b'0');
                        for _ in 0..level {
                            out.push('#');
                        }
                        out.push(' ');
                    }
                    "p" | "div" | "section" | "article" | "blockquote" | "pre" | "figure"
                    | "figcaption" => ensure_block(&mut out),
                    "ul" | "ol" => {
                        ensure_block(&mut out);
                        lists.push((tag == "ol", 0));
                    }
                    "li" => {
                        ensure_block(&mut out);
                        match lists.last_mut() {
                            Some((true, counter)) => {
                                *counter += 1;
                                out.push_str(&counter.to_string());
                                out.push_str(". ");
                            }
                            _ => out.push_str("- "),
                        }
                    }
                    "br" => out.push('\n'),
                    "hr" => {
                        ensure_block(&mut out);
                        out.push_str("---");
                    }
                    "strong" | "b" => out.push_str("**"),
                    "em" | "i" => out.push('*'),
                    "code" => out.push('`'),
                    "a" => {
                        out.push('[');
                        href = element.attr("href").map(str::to_owned);
                    }
                    "img" => {
                        let alt = element.attr("alt").unwrap_or("").trim();
                        let src = element.attr("src").unwrap_or("").trim();
                        out.push_str("![");
                        out.push_str(alt);
                        out.push_str("](");
                        out.push_str(src);
                        out.push(')');
                    }
                    _ => {}
                }
                if is_void(tag) || tag == "img" {
                    continue;
                }
                stack.push(MdStep::Close { tag, href });
                let children: Vec<_> = element.children().collect();
                for child in children.into_iter().rev() {
                    match child.value() {
                        Node::Text(text) => stack.push(MdStep::Text(text.to_string())),
                        Node::Element(_) => {
                            if let Some(child_element) = ElementRef::wrap(child) {
                                stack.push(MdStep::Open(child_element));
                            }
                        }
                        _ => {}
                    }
                }
            }
            MdStep::Text(text) => push_markdown_text(&mut out, &text),
            MdStep::Close { tag, href } => match tag {
                "strong" | "b" => out.push_str("**"),
                "em" | "i" => out.push('*'),
                "code" => out.push('`'),
                "a" => {
                    out.push_str("](");
                    out.push_str(href.as_deref().unwrap_or(""));
                    out.push(')');
                }
                "ul" | "ol" => {
                    lists.pop();
                    ensure_block(&mut out);
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "div" | "section" | "article"
                | "blockquote" | "pre" | "figure" | "figcaption" => ensure_block(&mut out),
                _ => {}
            },
        }
    }
    cleanup_markdown(&out)
}

/// 迭代序列化 body（剔除噪声子树），供回退路径做文本/markdown 渲染。
pub fn cleaned_body_html(document: &Html) -> String {
    serialize_skipping_noise(body_element(document))
}

enum MdStep<'a> {
    Open(ElementRef<'a>),
    Text(String),
    Close { tag: &'a str, href: Option<String> },
}

enum HtmlStep<'a> {
    Open(ElementRef<'a>),
    Text(String),
    Close(&'a str),
}

/// 显式栈的 HTML 序列化；遇到噪声标签整棵子树跳过。
pub fn serialize_skipping_noise(root: ElementRef<'_>) -> String {
    let mut out = String::new();
    let mut stack = vec![HtmlStep::Open(root)];
    while let Some(step) = stack.pop() {
        match step {
            HtmlStep::Open(element) => {
                let tag = element.value().name();
                if is_noise(tag) {
                    continue;
                }
                out.push('<');
                out.push_str(tag);
                for (key, value) in element.value().attrs() {
                    out.push(' ');
                    out.push_str(key);
                    out.push_str("=\"");
                    escape_attr(value, &mut out);
                    out.push('"');
                }
                out.push('>');
                if is_void(tag) {
                    continue;
                }
                stack.push(HtmlStep::Close(tag));
                let children: Vec<_> = element.children().collect();
                for child in children.into_iter().rev() {
                    match child.value() {
                        Node::Text(text) => stack.push(HtmlStep::Text(text.to_string())),
                        Node::Element(_) => {
                            if let Some(child_element) = ElementRef::wrap(child) {
                                stack.push(HtmlStep::Open(child_element));
                            }
                        }
                        _ => {}
                    }
                }
            }
            HtmlStep::Text(text) => escape_text(&text, &mut out),
            HtmlStep::Close(tag) => {
                out.push_str("</");
                out.push_str(tag);
                out.push('>');
            }
        }
    }
    out
}

fn escape_text(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
}

fn escape_attr(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
}

fn push_markdown_text(out: &mut String, raw: &str) {
    let leading = raw.starts_with(char::is_whitespace);
    let trailing = raw.ends_with(char::is_whitespace);
    let core = collapse_ws(raw);
    if core.is_empty() {
        return;
    }
    if leading && !out.is_empty() && !out.ends_with(' ') {
        out.push(' ');
    }
    out.push_str(&core);
    if trailing {
        out.push(' ');
    }
}

fn ensure_block(out: &mut String) {
    if out.is_empty() {
        return;
    }
    while out.ends_with([' ', '\t']) {
        out.pop();
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.ends_with("\n\n") {
        out.push('\n');
    }
}

fn cleanup_markdown(text: &str) -> String {
    let mut out = String::new();
    let mut pending_blank = false;
    for line in text.lines().map(str::trim_end) {
        if line.is_empty() {
            if !out.is_empty() {
                pending_blank = true;
            }
            continue;
        }
        if pending_blank && !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
        out.push('\n');
        pending_blank = false;
    }
    out.trim_end().to_string()
}
