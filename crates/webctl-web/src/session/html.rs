//! 会话引擎用的**最小 HTML 扫描器**（有意不做完整 DOM）。
//!
//! 边界（诚实声明）：本模块只服务会话/表单流，按标签线性扫描，不构建树、
//! 不做隐式闭合推断、不实现 CSS 选择器。完整的 HTML→DOM 属于 PR 03 解析域，
//! 本模块刻意与它解耦，避免 PR 04 依赖未合并的解析层。
//!
//! 覆盖：开始/结束标签、属性（引号/无引号）、注释、doctype、HTML 实体解码、
//! 原始文本元素（script/style 跳过内容，textarea/title 保留文本）。
//! 不覆盖：树结构、命名空间、模板语法、字符引用表全量。

/// 一个已解析的属性。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    /// 小写属性名。
    pub name: String,
    /// 已做实体解码的属性值（无值属性为空串）。
    pub value: String,
}

/// 一个开始或结束标签。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// 小写标签名。
    pub name: String,
    /// 属性（保序、允许重名）。
    pub attrs: Vec<Attribute>,
    /// 是否为结束标签（</tag>）。
    pub closing: bool,
    /// 是否自闭合（<br/> 或 void 元素）。
    pub self_closing: bool,
    /// < 在原文中的字节偏移。
    pub start: usize,
    /// > 之后一个字节的偏移。
    pub end: usize,
}

impl Tag {
    /// 取属性值（大小写不敏感，取首个）。
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|attr| attr.name == name)
            .map(|attr| attr.value.as_str())
    }

    /// 是否存在某属性。
    pub fn has_attr(&self, name: &str) -> bool {
        self.attrs.iter().any(|attr| attr.name == name)
    }
}

/// 线性节点流：标签或文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// 标签节点。
    Tag(Tag),
    /// 文本节点（已做实体解码）。
    Text(String),
}

/// 原始文本元素：内容不按标签解析。
fn is_raw_text(name: &str) -> bool {
    matches!(name, "script" | "style" | "textarea" | "title")
}

/// 内容被丢弃的原始文本元素（不产生文本节点）。
fn is_skipped_raw_text(name: &str) -> bool {
    matches!(name, "script" | "style")
}

/// void 元素：不会有结束标签。
fn is_void(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// 把 HTML 线性扫描为节点流。
pub fn tokenize(html: &str) -> Vec<Node> {
    let bytes = html.as_bytes();
    let mut nodes = Vec::new();
    let mut cursor = 0usize;
    let mut text_start = 0usize;
    let mut pending = String::new();

    fn flush(pending: &mut String, nodes: &mut Vec<Node>) {
        if !pending.is_empty() {
            nodes.push(Node::Text(std::mem::take(pending)));
        }
    }

    while cursor < bytes.len() {
        if bytes[cursor] != b'<' {
            cursor += 1;
            continue;
        }
        if html[cursor..].starts_with("<!--") {
            pending.push_str(&html[text_start..cursor]);
            flush(&mut pending, &mut nodes);
            cursor = find_ignore_ascii_case(html, cursor + 4, "-->")
                .map(|index| index + 3)
                .unwrap_or(bytes.len());
            text_start = cursor;
            continue;
        }
        if html[cursor..].starts_with("<!") || html[cursor..].starts_with("<?") {
            pending.push_str(&html[text_start..cursor]);
            flush(&mut pending, &mut nodes);
            cursor = html[cursor..]
                .find('>')
                .map(|offset| cursor + offset + 1)
                .unwrap_or(bytes.len());
            text_start = cursor;
            continue;
        }
        let Some((tag, next)) = parse_tag(html, cursor) else {
            cursor += 1;
            continue;
        };
        pending.push_str(&html[text_start..cursor]);
        flush(&mut pending, &mut nodes);

        let raw = !tag.closing && is_raw_text(&tag.name);
        let skip = !tag.closing && is_skipped_raw_text(&tag.name);
        nodes.push(Node::Tag(tag.clone()));
        cursor = next;
        text_start = cursor;

        if raw {
            let closing = format!("</{}", tag.name);
            let (content, after) = match find_ignore_ascii_case(html, cursor, &closing) {
                Some(index) => {
                    let after = html[index..]
                        .find('>')
                        .map(|offset| index + offset + 1)
                        .unwrap_or(bytes.len());
                    (&html[cursor..index], after)
                }
                None => (&html[cursor..], bytes.len()),
            };
            if !skip {
                let decoded = decode_entities(content);
                if !decoded.is_empty() {
                    nodes.push(Node::Text(decoded));
                }
            }
            cursor = after;
            text_start = cursor;
        }
    }
    pending.push_str(&html[text_start..]);
    flush(&mut pending, &mut nodes);
    nodes
}

/// 解析一个标签，返回标签与下一个偏移；失败返回 None。
fn parse_tag(html: &str, start: usize) -> Option<(Tag, usize)> {
    let bytes = html.as_bytes();
    let mut cursor = start + 1;
    let closing = bytes.get(cursor) == Some(&b'/');
    if closing {
        cursor += 1;
    }
    let name_start = cursor;
    while cursor < bytes.len() && is_name_byte(bytes[cursor]) {
        cursor += 1;
    }
    if cursor == name_start {
        return None;
    }
    let name = html[name_start..cursor].to_ascii_lowercase();

    let mut attrs = Vec::new();
    let mut self_closing = false;
    loop {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        match bytes.get(cursor) {
            None => break,
            Some(b'>') => {
                cursor += 1;
                break;
            }
            Some(b'/') => {
                self_closing = true;
                cursor += 1;
            }
            Some(_) => {
                let Some((attr, next)) = parse_attribute(html, cursor) else {
                    cursor += 1;
                    continue;
                };
                attrs.push(attr);
                cursor = next;
            }
        }
    }
    if is_void(&name) {
        self_closing = true;
    }
    Some((
        Tag {
            name,
            attrs,
            closing,
            self_closing,
            start,
            end: cursor,
        },
        cursor,
    ))
}

fn parse_attribute(html: &str, start: usize) -> Option<(Attribute, usize)> {
    let bytes = html.as_bytes();
    let mut cursor = start;
    let name_start = cursor;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if byte.is_ascii_whitespace() || byte == b'=' || byte == b'>' || byte == b'/' {
            break;
        }
        cursor += 1;
    }
    if cursor == name_start {
        return None;
    }
    let name = html[name_start..cursor].to_ascii_lowercase();
    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    let mut value = String::new();
    if bytes.get(cursor) == Some(&b'=') {
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        match bytes.get(cursor) {
            Some(&quote @ (b'"' | b'\'')) => {
                cursor += 1;
                let value_start = cursor;
                while cursor < bytes.len() && bytes[cursor] != quote {
                    cursor += 1;
                }
                value = decode_entities(&html[value_start..cursor]);
                if cursor < bytes.len() {
                    cursor += 1;
                }
            }
            Some(_) => {
                let value_start = cursor;
                while cursor < bytes.len()
                    && !bytes[cursor].is_ascii_whitespace()
                    && bytes[cursor] != b'>'
                {
                    cursor += 1;
                }
                value = decode_entities(&html[value_start..cursor]);
            }
            None => {}
        }
    }
    Some((Attribute { name, value }, cursor))
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.')
}

/// ASCII 大小写不敏感的子串查找。
pub fn find_ignore_ascii_case(haystack: &str, from: usize, needle: &str) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() {
        return None;
    }
    let hay = haystack.as_bytes();
    let needle_bytes = needle.as_bytes();
    let first = needle_bytes[0].to_ascii_lowercase();
    let mut index = from;
    while index + needle_bytes.len() <= hay.len() {
        if hay[index].to_ascii_lowercase() == first
            && hay[index..index + needle_bytes.len()]
                .iter()
                .zip(needle_bytes)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            return Some(index);
        }
        index += 1;
    }
    None
}

/// 解码常见 HTML 实体；未知实体原样保留。
pub fn decode_entities(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] != b'&' {
            let start = cursor;
            while cursor < bytes.len() && bytes[cursor] != b'&' {
                cursor += 1;
            }
            output.push_str(&input[start..cursor]);
            continue;
        }
        let Some(semicolon) = input[cursor..].find(';').map(|offset| cursor + offset) else {
            output.push('&');
            cursor += 1;
            continue;
        };
        match decode_entity(&input[cursor + 1..semicolon]) {
            Some(decoded) => {
                output.push(decoded);
                cursor = semicolon + 1;
            }
            None => {
                output.push('&');
                cursor += 1;
            }
        }
    }
    output
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => {
            let number = entity.strip_prefix('#')?;
            let code = if let Some(hex) = number.strip_prefix(['x', 'X']) {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                number.parse::<u32>().ok()?
            };
            char::from_u32(code)
        }
    }
}

/// 文本节点拼接后的可见文本。
pub fn text_content(nodes: &[Node]) -> String {
    let mut output = String::new();
    for node in nodes {
        if let Node::Text(text) = node {
            output.push_str(text);
            output.push(' ');
        }
    }
    output
}

/// 统计某标签出现次数（不含结束标签）。
pub fn count_tags(nodes: &[Node], name: &str) -> usize {
    nodes
        .iter()
        .filter(|node| matches!(node, Node::Tag(tag) if !tag.closing && tag.name == name))
        .count()
}

/// script 标签数量与脚本正文总字节数。
pub fn script_stats(html: &str) -> (usize, usize) {
    let mut count = 0usize;
    let mut bytes = 0usize;
    let mut cursor = 0usize;
    while let Some(start) = find_ignore_ascii_case(html, cursor, "<script") {
        let Some(open_end) = html[start..].find('>').map(|offset| start + offset + 1) else {
            break;
        };
        let Some(close) = find_ignore_ascii_case(html, open_end, "</script") else {
            bytes += html.len().saturating_sub(open_end);
            count += 1;
            break;
        };
        count += 1;
        bytes += close.saturating_sub(open_end);
        cursor = close + 1;
    }
    (count, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_tags_attributes_and_text() {
        let html = r#"<form action="/x" method="POST"><input type=text name="a" value='b&amp;c'>hi</form>"#;
        let nodes = tokenize(html);
        let tags: Vec<&Tag> = nodes
            .iter()
            .filter_map(|node| match node {
                Node::Tag(tag) => Some(tag),
                Node::Text(_) => None,
            })
            .collect();
        assert_eq!(tags[0].name, "form");
        assert_eq!(tags[0].attr("action"), Some("/x"));
        assert_eq!(tags[0].attr("method"), Some("POST"));
        assert_eq!(tags[1].name, "input");
        assert!(tags[1].self_closing, "input 是 void 元素");
        assert_eq!(tags[1].attr("value"), Some("b&c"));
        assert_eq!(tags[2].name, "form");
        assert!(tags[2].closing);
    }

    #[test]
    fn skips_comments_and_raw_script() {
        let html = "<!-- <input name=x> --><script>var a = '<input name=y>';</script><p>ok</p>";
        let nodes = tokenize(html);
        assert_eq!(
            count_tags(&nodes, "input"),
            0,
            "注释与脚本内的 input 不算数"
        );
        assert!(text_content(&nodes).contains("ok"));
        let (count, bytes) = script_stats(html);
        assert_eq!(count, 1);
        assert!(bytes > 0);
    }

    #[test]
    fn textarea_content_is_kept() {
        let nodes = tokenize("<textarea name=\"t\">&lt;hi&gt; a&amp;b</textarea>");
        let texts: Vec<&str> = nodes
            .iter()
            .filter_map(|node| match node {
                Node::Text(text) => Some(text.as_str()),
                Node::Tag(_) => None,
            })
            .collect();
        assert_eq!(texts, vec!["<hi> a&b"]);
    }

    #[test]
    fn entities_decode() {
        assert_eq!(decode_entities("a&lt;b&amp;c&#39;d&#x41;"), "a<b&c'dA");
        assert_eq!(decode_entities("&unknown;"), "&unknown;");
    }
}
