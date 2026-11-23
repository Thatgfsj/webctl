//! 文本上下文索引（EXTRACT-02）。
//!
//! 给文档里的每个元素记录它在**扁平文本流**里的字符区间，命中时取区间前后的
//! 若干字符作为上下文。扁平流按文档序拼接文本节点，节点之间补一个空格，最后
//! 折叠空白，因此上下文是稳定可复现的纯文本，不含标签或原始字节。
//!
//! 不是所有文本节点都算正文：`script` / `style` / `template` 子树是脚本源码、
//! 样式源码或未渲染的模板，拼进扁平流会让上下文（以及调用方的命中文本）变成
//! 不可信的脚本源码。这些子树的文本在索引时直接跳过；元素索引与区间仍按文档序
//! 保留（被跳过子树的元素拿到零宽区间），因此 `elements` / `ranges` 与 `text`
//! 始终一致。`noscript` 不跳过：它是可见的静态回退内容。

use scraper::{ElementRef, Html, Node};

/// 扁平文本流与命中文本里跳过其文本的子树。
///
/// 与 `parse::html::NOISE_TAGS` 不同：这里只回答「哪些文本节点不是正文」，不涉及
/// 导航/表单等可选内容，因此 `noscript` 保留。
const SKIPPED_TEXT_TAGS: &[&str] = &["script", "style", "template"];

/// 该标签的子树文本是否必须排除在正文之外。
pub fn skips_text(tag: &str) -> bool {
    SKIPPED_TEXT_TAGS.contains(&tag)
}

/// 元素后代文本（文档序），跳过 `script` / `style` / `template` 子树。
///
/// 与 `ElementRef::text()` 的语义对齐：文本节点直接拼接、不补分隔符，调用方自行
/// 折叠空白；区别只在于不收脚本/样式/未渲染模板的文本。显式栈遍历，深度不受
/// 递归限制。
pub fn visible_text(element: &ElementRef<'_>) -> String {
    enum Step<'a> {
        Open(ElementRef<'a>),
        Text(String),
    }
    let mut text = String::new();
    let mut stack = vec![Step::Open(*element)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Open(current) => {
                if skips_text(current.value().name()) {
                    continue;
                }
                let children: Vec<_> = current.children().collect();
                for child in children.into_iter().rev() {
                    match child.value() {
                        Node::Text(value) => stack.push(Step::Text(value.to_string())),
                        Node::Element(_) => {
                            if let Some(child_element) = ElementRef::wrap(child)
                                && !skips_text(child_element.value().name())
                            {
                                stack.push(Step::Open(child_element));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Step::Text(value) => text.push_str(&value),
        }
    }
    text
}

/// 一次解析的扁平文本索引。
pub struct DocumentText<'a> {
    text: String,
    elements: Vec<ElementRef<'a>>,
    ranges: Vec<(usize, usize)>,
}

impl<'a> DocumentText<'a> {
    /// 以显式栈的文档序遍历建立索引，深度不受递归限制。
    pub fn build(document: &'a Html) -> Self {
        enum Step<'a> {
            Open(ElementRef<'a>, bool),
            Text(String),
            Close(usize),
        }
        let mut text = String::new();
        let mut elements: Vec<ElementRef<'a>> = Vec::new();
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        let mut stack = vec![Step::Open(document.root_element(), false)];
        while let Some(step) = stack.pop() {
            match step {
                Step::Open(element, skip_text) => {
                    let index = elements.len();
                    elements.push(element);
                    ranges.push((text.len(), 0));
                    stack.push(Step::Close(index));
                    // 当前元素或其祖先被跳过时，整棵子树的文本都不进入扁平流；
                    // 元素本身仍然入索引，区间退化为 [len, len)，保证元素与区间一一对应。
                    let skip_children = skip_text || skips_text(element.value().name());
                    let children: Vec<_> = element.children().collect();
                    for child in children.into_iter().rev() {
                        match child.value() {
                            Node::Text(value) => {
                                if !skip_children {
                                    stack.push(Step::Text(value.to_string()));
                                }
                            }
                            Node::Element(_) => {
                                if let Some(child_element) = ElementRef::wrap(child) {
                                    stack.push(Step::Open(child_element, skip_children));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Step::Text(value) => {
                    // 纯空白文本节点（源码缩进）不进入扁平流，否则会把有意义的
                    // 上下文挤出窗口；非空文本节点后补一个空格以分隔相邻元素。
                    if !value.trim().is_empty() {
                        text.push_str(&value);
                        text.push(' ');
                    }
                }
                Step::Close(index) => ranges[index].1 = text.len(),
            }
        }
        Self {
            text,
            elements,
            ranges,
        }
    }

    /// 元素子树的字符区间 [start, end)。
    pub fn range_of(&self, element: &ElementRef<'_>) -> Option<(usize, usize)> {
        let id = element.id();
        self.elements
            .iter()
            .position(|candidate| candidate.id() == id)
            .map(|index| self.ranges[index])
    }

    /// 区间起点前至多 count 个字符。
    pub fn before(&self, start: usize, count: usize) -> String {
        tail_chars(&self.text[..start], count)
    }

    /// 区间终点后至多 count 个字符。
    pub fn after(&self, end: usize, count: usize) -> String {
        head_chars(&self.text[end..], count)
    }
}

/// 取字符串开头至多 count 个字符（UTF-8 安全）。
pub fn head_chars(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

/// 取字符串末尾至多 count 个字符（UTF-8 安全）。
pub fn tail_chars(text: &str, count: usize) -> String {
    let total = text.chars().count();
    text.chars().skip(total.saturating_sub(count)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::html::{collapse_ws, parse_document, selector};

    fn select<'a>(document: &'a Html, css: &str) -> ElementRef<'a> {
        let css = selector(css).expect("static selector");
        document.select(&css).next().expect("fixture element")
    }

    /// #188：扁平文本流跳过 script/style/template 文本，保留 noscript（可见回退）。
    #[test]
    fn flat_text_skips_script_style_template_and_keeps_noscript() {
        let document = parse_document(
            "<html><body><main id=\"c\"><p>VISIBLE</p>\
             <script>var SECRET = 1;</script>\
             <style>.leak { color: red; }</style>\
             <template><p>TEMPLATED</p></template>\
             <noscript>NOSCRIPT-FALLBACK</noscript><p>TAIL</p></main></body></html>",
        );
        let index = DocumentText::build(&document);
        let main = select(&document, "main");
        let (start, end) = index.range_of(&main).expect("main is indexed");
        let text = collapse_ws(&index.text[start..end]);
        assert_eq!(text, "VISIBLE NOSCRIPT-FALLBACK TAIL");
        for leaked in ["SECRET", "color: red", "TEMPLATED"] {
            assert!(!text.contains(leaked), "leaked {leaked:?} in {text:?}");
        }

        // 区间不变量：元素遍历可达的每个元素都入索引，区间落在扁平流内且 start <= end。
        // （template 内容挂在 Fragment 节点下，不属于可渲染元素遍历，故不在此列。）
        let mut stack = vec![document.root_element()];
        while let Some(element) = stack.pop() {
            let (start, end) = index
                .range_of(&element)
                .expect("reachable element is indexed");
            assert!(
                start <= end,
                "range {start}..{end} for {:?}",
                element.value()
            );
            assert!(end <= index.text.len());
            for child in element.children() {
                if let Some(child_element) = ElementRef::wrap(child) {
                    stack.push(child_element);
                }
            }
        }
    }

    /// #188：被跳过子树的元素仍可取区（零宽），且 before/after 只含可见文本。
    #[test]
    fn skipped_element_has_empty_range_and_visible_context() {
        let document = parse_document(
            "<html><body><p>BEFORE</p><script>window.LEAK = 1;</script><p>AFTER</p></body></html>",
        );
        let index = DocumentText::build(&document);
        let script = select(&document, "script");
        let (start, end) = index.range_of(&script).expect("script is indexed");
        assert_eq!(start, end, "script text must not enter the flat stream");
        assert!(!index.text.contains("LEAK"), "{}", index.text);
        assert!(index.before(start, 40).contains("BEFORE"));
        assert!(index.after(end, 40).contains("AFTER"));
    }

    /// 回归：没有跳过子树时，visible_text 与 ElementRef::text() 逐字节一致
    /// （相邻文本节点之间不补分隔符）。
    #[test]
    fn visible_text_matches_element_text_without_skipped_subtrees() {
        let document = parse_document("<body><div id=\"d\">a<span>b</span>c</div></body>");
        let div = select(&document, "div");
        let native: String = div.text().collect();
        assert_eq!(visible_text(&div), native);
        assert_eq!(visible_text(&div), "abc");
    }

    /// #188：嵌套的 script/template/style 子树整体跳过，noscript 保留；
    /// 直接命中 script 时文本为空（脚本源码不是正文）。
    #[test]
    fn visible_text_skips_nested_skipped_subtrees() {
        let document = parse_document(
            "<body><div id=\"d\">A<script>S</script><template><p>T</p></template>\
             <style>.C{}</style><noscript>N</noscript>B</div></body>",
        );
        let div = select(&document, "div");
        assert_eq!(visible_text(&div), "ANB");
        let script = select(&document, "script");
        assert_eq!(visible_text(&script), "");
    }
}
