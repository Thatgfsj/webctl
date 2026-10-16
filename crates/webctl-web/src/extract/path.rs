//! 结构化路径（PARSE-12 / EXTRACT-01）。
//!
//! 路径是**单次解析内**的坐标，不是跨请求引用：同一选择器下次抓取可能命中
//! 别的元素（见 crate::extract 的稳定性语义）。路径能唯一定位当前文档树里的
//! 一个元素：tag + 每个祖先的 tag/同级索引 + 命中自身的同级索引。

use scraper::ElementRef;

use super::PathSegment;

/// 计算元素的结构化路径。
pub fn element_path(element: &ElementRef<'_>) -> super::ElementPath {
    let tag = element.value().name().to_ascii_lowercase();
    let sibling_index = sibling_index_of(element);
    let mut ancestors: Vec<PathSegment> = Vec::new();
    let mut current = element.parent().and_then(ElementRef::wrap);
    while let Some(parent) = current {
        ancestors.push(PathSegment {
            tag: parent.value().name().to_ascii_lowercase(),
            sibling_index: sibling_index_of(&parent),
        });
        current = parent.parent().and_then(ElementRef::wrap);
    }
    ancestors.reverse();
    let css = render_css(&tag, sibling_index, &ancestors);
    let depth = ancestors.len();
    super::ElementPath {
        tag,
        sibling_index,
        ancestors,
        depth,
        css,
    }
}

/// 元素在同级**元素**中的 0 起索引（忽略文本/注释节点）。
fn sibling_index_of(element: &ElementRef<'_>) -> usize {
    let id = element.id();
    match element.parent() {
        Some(parent) => parent
            .children()
            .filter_map(ElementRef::wrap)
            .take_while(|sibling| sibling.id() != id)
            .count(),
        None => 0,
    }
}

/// 渲染可复现的 CSS 路径；每段都带 nth-of-type，避免动态 class 影响。
fn render_css(tag: &str, sibling_index: usize, ancestors: &[PathSegment]) -> String {
    let mut css = String::new();
    for segment in ancestors {
        push_segment(&mut css, &segment.tag, segment.sibling_index);
    }
    push_segment(&mut css, tag, sibling_index);
    css
}

fn push_segment(css: &mut String, tag: &str, sibling_index: usize) {
    if !css.is_empty() {
        css.push_str(" > ");
    }
    css.push_str(tag);
    css.push_str(":nth-of-type(");
    css.push_str(&(sibling_index + 1).to_string());
    css.push(')');
}
