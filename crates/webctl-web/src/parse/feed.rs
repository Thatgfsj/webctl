//! RSS 2.0 / Atom 1.0 的最小解析（议题 #117 第 2 步）。
//!
//! 只做"站点声明了 feed、我们礼貌地取回来看看有什么"这件事，**不**追求完整 feed 规范：
//! 认不清的结构一律跳过并在 `note` 里说明，**不编造**条目。
//!
//! 与元数据通道的边界（#117 验收）：这里返回的是**条目**（标题/链接/时间），
//! 调用方必须按 `source="feed"` 区分，不得并进"正文"。

use serde::{Deserialize, Serialize};

/// 一条 feed 条目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedItem {
    /// 条目标题（XML 文本，已去标签与实体还原）。
    pub title: String,
    /// 条目链接（RSS 的 `<link>` 或 Atom 的 `<link href>`；**原样**，相对路径由调用方解析）。
    pub link: Option<String>,
    /// 发布时间原样（RSS 的 `<pubDate>` / Atom 的 `<updated>` 或 `<published>`）。
    pub published: Option<String>,
}

/// 解析结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedReport {
    /// 通道标题（RSS `<channel><title>` 或 Atom `<feed><title>`）。
    pub title: Option<String>,
    /// 条目（保 feed 顺序，最多 `max_items` 条）。
    pub items: Vec<FeedItem>,
    /// 是否因条数上限被截断。
    pub truncated: bool,
    /// 可读说明（认不出的结构写在这里）。
    pub note: String,
}

/// 解析上限：条目数（调用方还可再截）。
pub const MAX_FEED_ITEMS: usize = 50;

/// 解析 RSS 或 Atom；认不出格式时 `items` 为空、`note` 说明原因（**不是错误**：
/// "取回来了但不是可识别的 feed"与"取不到"是两件事，由上层区分）。
pub fn parse_feed(xml: &str) -> FeedReport {
    let lower = xml.to_ascii_lowercase();
    let (kind, item_tags) = if lower.contains("<rss") {
        ("rss", ["item", "item"])
    } else if lower.contains("<feed") {
        ("atom", ["entry", "entry"])
    } else {
        return FeedReport {
            title: None,
            items: Vec::new(),
            truncated: false,
            note: "既不是 RSS（缺 <rss>）也不是 Atom（缺 <feed>）".to_owned(),
        };
    };
    let title = first_text(xml, "title").filter(|text| !text.is_empty());
    let mut items = Vec::new();
    let mut offset = 0usize;
    while let Some(start) = find_open(&lower, item_tags[0], offset) {
        let Some(end) = find_close(&lower, item_tags[1], start) else {
            break;
        };
        let block = &xml[start..end];
        let item_title = first_text(block, "title").unwrap_or_default();
        let link = if kind == "atom" {
            // Atom：<link href="…"/>（优先 rel="alternate"，没有 rel 也算）。
            first_attr(block, "link", "href")
        } else {
            first_text(block, "link").filter(|text| !text.is_empty())
        };
        let published = first_text(block, "pubdate")
            .or_else(|| first_text(block, "published"))
            .or_else(|| first_text(block, "updated"));
        if item_title.is_empty() && link.is_none() {
            offset = end;
            continue;
        }
        items.push(FeedItem {
            title: item_title,
            link,
            published,
        });
        if items.len() >= MAX_FEED_ITEMS {
            let truncated = find_open(&lower, item_tags[0], end).is_some();
            return FeedReport {
                title,
                items,
                truncated,
                note: format!("{kind} feed；已达解析上限 {MAX_FEED_ITEMS} 条"),
            };
        }
        offset = end;
    }
    let note = if items.is_empty() {
        format!("{kind} feed 里没有可识别条目（结构不完整或为空）")
    } else {
        format!("{kind} feed，{} 条", items.len())
    };
    FeedReport {
        title,
        items,
        truncated: false,
        note,
    }
}

/// `<name>` 的起始位置（大小写不敏感，要求后面跟空白或 `>`，避免 `<metadata>` 命中了 `<meta`）。
fn find_open(lower: &str, name: &str, from: usize) -> Option<usize> {
    let needle = format!("<{name}");
    let mut offset = from;
    while let Some(position) = lower[offset..].find(&needle) {
        let at = offset + position;
        let after = lower[at + needle.len()..].chars().next();
        let boundary = matches!(after, Some(ch) if ch == '>' || ch.is_whitespace() || ch == '/');
        if boundary {
            return Some(at);
        }
        offset = at + needle.len();
    }
    None
}

/// `</name>` 的起始位置。
fn find_close(lower: &str, name: &str, from: usize) -> Option<usize> {
    let needle = format!("</{name}>");
    lower[from..].find(&needle).map(|position| from + position)
}

/// 第一个 `<name>…</name>` 的**纯文本**（去标签、实体还原、压缩空白）。
fn first_text(xml: &str, name: &str) -> Option<String> {
    let lower = xml.to_ascii_lowercase();
    let start = find_open(&lower, name, 0)?;
    let after = lower[start..].find('>')? + start + 1;
    let end = find_close(&lower, name, after)?;
    let text = strip_tags(&xml[after..end]);
    let text = super::embedded::unescape_entities(&text);
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(collapsed.trim().to_owned())
}

/// 某个标签上的属性值（例如 Atom 的 `<link rel="alternate" href="…">`）。
fn first_attr(xml: &str, tag: &str, attr: &str) -> Option<String> {
    let lower = xml.to_ascii_lowercase();
    let start = find_open(&lower, tag, 0)?;
    let end = lower[start..].find('>')? + start;
    let tag_text = &xml[start..=end];
    super::site_channels::attr(tag_text, attr)
}

/// 去掉内部标签，只留文本节点。
fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for ch in text.chars() {
        match ch {
            '<' => inside = true,
            '>' => inside = false,
            other if !inside => out.push(other),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?><rss version="2.0"><channel>
        <title>Example Feed</title>
        <link>https://example.test/</link>
        <item><title>First &amp; best</title><link>https://example.test/a</link><pubDate>Mon, 01 Jan 2024 00:00:00 GMT</pubDate></item>
        <item><title>Second</title><link>https://example.test/b</link></item>
        </channel></rss>"#;

    const ATOM: &str = r#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom">
        <title>Atom Feed</title>
        <entry><title>Entry one</title><link rel="alternate" href="https://example.test/1"/><updated>2024-01-01T00:00:00Z</updated></entry>
        </feed>"#;

    #[test]
    fn parses_rss_items_in_order() {
        let report = parse_feed(RSS);
        assert_eq!(report.title.as_deref(), Some("Example Feed"));
        assert_eq!(report.items.len(), 2);
        assert_eq!(report.items[0].title, "First & best");
        assert_eq!(
            report.items[0].link.as_deref(),
            Some("https://example.test/a")
        );
        assert!(report.items[0].published.is_some());
        assert_eq!(report.items[1].title, "Second");
    }

    #[test]
    fn parses_atom_links_via_href() {
        let report = parse_feed(ATOM);
        assert_eq!(report.title.as_deref(), Some("Atom Feed"));
        assert_eq!(report.items.len(), 1);
        assert_eq!(
            report.items[0].link.as_deref(),
            Some("https://example.test/1")
        );
        assert_eq!(
            report.items[0].published.as_deref(),
            Some("2024-01-01T00:00:00Z")
        );
    }

    #[test]
    fn unknown_document_is_empty_with_note() {
        let report = parse_feed("<html><body>no feed</body></html>");
        assert!(report.items.is_empty());
        assert!(report.note.contains("既不是 RSS"));
    }

    #[test]
    fn empty_feed_is_not_an_error() {
        let report = parse_feed("<rss><channel><title>Empty</title></channel></rss>");
        assert!(report.items.is_empty());
        assert!(report.note.contains("没有可识别条目"));
    }

    #[test]
    fn entity_decoded_once() {
        let report = parse_feed("<rss><channel><title>a&amp;lt;b</title></channel></rss>");
        assert_eq!(report.title.as_deref(), Some("a&lt;b"));
    }
}
