//! 分页 next 链接识别（PLAN v0.4 §2「会话」域，PR 04）。
//!
//! 识别顺序（确定性）：
//! 1. link[rel=next][href]；
//! 2. a[rel=next][href]；
//! 3. a[href]，其文本是 next / “下一页” / › / » 等。
//!
//! 只做识别与相对 URL 解析；遍历、页数上限与循环终止在 engine::paginate。

use serde::{Deserialize, Serialize};
use url::Url;

use super::html::{self, Node};

/// next 链接的识别来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NextSource {
    /// link rel=next。
    LinkRel,
    /// a rel=next。
    AnchorRel,
    /// a 文本匹配。
    AnchorText,
}

/// 识别到的 next 链接。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextLink {
    /// 解析后的绝对 URL。
    pub url: String,
    /// 链接文本（若有）。
    pub text: Option<String>,
    /// 识别来源。
    pub source: NextSource,
}

/// 在页面里寻找下一页链接。base 用于把相对 href 解析为绝对 URL。
pub fn find_next(html: &str, base: &Url) -> Option<NextLink> {
    let nodes = html::tokenize(html);

    for node in &nodes {
        let Node::Tag(tag) = node else { continue };
        if tag.closing || tag.name != "link" {
            continue;
        }
        if rel_has_next(tag.attr("rel"))
            && let Some(link) = resolve(tag.attr("href"), base, None, NextSource::LinkRel)
        {
            return Some(link);
        }
    }

    for (index, node) in nodes.iter().enumerate() {
        let Node::Tag(tag) = node else { continue };
        if tag.closing || tag.name != "a" {
            continue;
        }
        let text = anchor_text(&nodes, index);
        let text_ref = text.as_deref();
        if rel_has_next(tag.attr("rel"))
            && let Some(link) = resolve(tag.attr("href"), base, text_ref, NextSource::AnchorRel)
        {
            return Some(link);
        }
        if is_next_text(text_ref)
            && let Some(link) = resolve(tag.attr("href"), base, text_ref, NextSource::AnchorText)
        {
            return Some(link);
        }
    }
    None
}

fn rel_has_next(rel: Option<&str>) -> bool {
    rel.is_some_and(|rel| {
        rel.split_whitespace()
            .any(|token| token.eq_ignore_ascii_case("next"))
    })
}

fn is_next_text(text: Option<&str>) -> bool {
    let Some(text) = text else { return false };
    let normalized = normalize(text);
    normalized == "next"
        || normalized.starts_with("next ")
        || normalized == "next page"
        || normalized == "›"
        || normalized == "»"
        || normalized == "→"
        || normalized == "下一页"
        || normalized == "下页"
        || normalized == "次へ"
}

fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn anchor_text(nodes: &[Node], start: usize) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for node in &nodes[start + 1..] {
        match node {
            Node::Tag(tag) if tag.closing && tag.name == "a" => break,
            Node::Tag(tag)
                if !tag.closing && matches!(tag.name.as_str(), "a" | "script" | "style") =>
            {
                break;
            }
            Node::Text(text) => parts.push(text),
            _ => {}
        }
    }
    let joined = parts.join(" ");
    let trimmed = joined.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn resolve(
    href: Option<&str>,
    base: &Url,
    text: Option<&str>,
    source: NextSource,
) -> Option<NextLink> {
    let href = href?.trim();
    if href.is_empty()
        || href.starts_with('#')
        || href.to_ascii_lowercase().starts_with("javascript:")
    {
        return None;
    }
    let url = base.join(href).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    Some(NextLink {
        url: url.to_string(),
        text: text.map(str::to_owned),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("http://example.test/list?page=1").expect("url")
    }

    #[test]
    fn prefers_link_rel_next() {
        let html = r#"<html><head><link rel="next" href="/list?page=2"></head>
            <body><a rel="next" href="/list?page=3">Next</a></body></html>"#;
        let next = find_next(html, &base()).expect("next");
        assert_eq!(next.source, NextSource::LinkRel);
        assert_eq!(next.url, "http://example.test/list?page=2");
    }

    #[test]
    fn recognizes_anchor_rel_and_text() {
        let html = r#"<a rel="next" href="?page=2">继续</a>"#;
        let next = find_next(html, &base()).expect("next");
        assert_eq!(next.source, NextSource::AnchorRel);
        assert_eq!(next.url, "http://example.test/list?page=2");

        let html = r#"<a href="?page=7">下一页</a>"#;
        let next = find_next(html, &base()).expect("next");
        assert_eq!(next.source, NextSource::AnchorText);
        assert_eq!(next.url, "http://example.test/list?page=7");
    }

    #[test]
    fn ignores_non_next_and_js_links() {
        let html =
            r##"<a href="/x">上一页</a><a href="javascript:void(0)">Next</a><a href="#">Next</a>"##;
        assert!(find_next(html, &base()).is_none());
    }
}
