//! 文本上下文索引（EXTRACT-02）。
//!
//! 给文档里的每个元素记录它在**扁平文本流**里的字符区间，命中时取区间前后的
//! 若干字符作为上下文。扁平流按文档序拼接文本节点，节点之间补一个空格，最后
//! 折叠空白，因此上下文是稳定可复现的纯文本，不含标签或原始字节。

use scraper::{ElementRef, Html, Node};

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
            Open(ElementRef<'a>),
            Text(String),
            Close(usize),
        }
        let mut text = String::new();
        let mut elements: Vec<ElementRef<'a>> = Vec::new();
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        let mut stack = vec![Step::Open(document.root_element())];
        while let Some(step) = stack.pop() {
            match step {
                Step::Open(element) => {
                    let index = elements.len();
                    elements.push(element);
                    ranges.push((text.len(), 0));
                    stack.push(Step::Close(index));
                    let children: Vec<_> = element.children().collect();
                    for child in children.into_iter().rev() {
                        match child.value() {
                            Node::Text(value) => stack.push(Step::Text(value.to_string())),
                            Node::Element(_) => {
                                if let Some(child_element) = ElementRef::wrap(child) {
                                    stack.push(Step::Open(child_element));
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
