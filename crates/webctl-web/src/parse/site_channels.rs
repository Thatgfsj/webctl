//! 站点级替代通道与页面元数据的**离线抽取**（议题 #117 第 1 步；**尚未接线**到工具）。
//!
//! 分工：本模块只做**纯离线的解析**——从一个已经拿到的 HTML 文本里找出
//! ①`<link rel="alternate">` 声明的 RSS/Atom/oEmbed 通道；②`og:*` / `twitter:*` /
//! `<title>` / `<link rel="canonical">` 这类元数据。
//! **不做任何网络请求**（那属于第 2 步，且必须先过 politeness）。
//!
//! 诚实性约束（对应议题 #117 的验收）：
//! - 每条元数据都带 **`source`**（`og-meta` / `twitter-meta` / `html-meta`），
//!   调用方据此区分"这是元数据"还是"这是正文"——**不得把元数据冒充正文**。
//! - 找不到就是**空**（`entries` 为空、`title/canonical` 为 None），不是错误、也不猜。
//! - 通道只报告**声明**，不判断可达性；未来接线时"声明了但拉不到"必须单独降级。

use serde::{Deserialize, Serialize};

/// 元数据的来源标签。**必须**如实标注，不能与正文混同。
pub const SOURCE_OG: &str = "og-meta";
/// Twitter card 元数据。
pub const SOURCE_TWITTER: &str = "twitter-meta";
/// 页面自身的 HTML 元数据（`<title>` / `<link rel=canonical>`）。
pub const SOURCE_HTML: &str = "html-meta";

/// 一条元数据（`source` 说明它来自哪一类，绝不是正文）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataEntry {
    /// 来源标签，取值见 `SOURCE_*`。
    pub source: String,
    /// 键名（如 `og:title`、`twitter:card`、`title`、`canonical`）。
    pub key: String,
    /// 值（已做实体解码与去空白）。
    pub content: String,
}

/// 站点级元数据汇总。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteMetadata {
    /// `<title>` 的文本（去空白）；缺失为 `None`。
    pub title: Option<String>,
    /// `<link rel="canonical">` 的 href；缺失为 `None`。
    pub canonical: Option<String>,
    /// 全部元数据条目（保序：og → twitter → html）。
    pub entries: Vec<MetadataEntry>,
}

/// 站点级**声明**的通道类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelKind {
    /// `<link rel="alternate" type="application/rss+xml">`
    Rss,
    /// `<link rel="alternate" type="application/atom+xml">`
    Atom,
    /// `<link rel="alternate" type="application/json+oembed">`
    OEmbed,
}

/// 一条**被声明的**通道（本步不拉取，只报告声明）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// 通道类型。
    pub kind: ChannelKind,
    /// 声明的 href（**原样**，不做相对路径解析——那需要文档 URL，属接线步骤）。
    pub href: String,
    /// `title` 属性（若有）。
    pub title: Option<String>,
}

/// 抽取页面元数据。找不到任何元数据时返回空结构（**不是错误**）。
pub fn site_metadata(html: &str) -> SiteMetadata {
    let mut metadata = SiteMetadata::default();
    for attrs in tags(html, "meta") {
        // og:* 用 property，twitter:* 两者都有（name 更常见）。
        let key = attr(&attrs, "property").or_else(|| attr(&attrs, "name"));
        let Some(key) = key else { continue };
        let Some(content) = attr(&attrs, "content") else {
            continue;
        };
        let source = if key.starts_with("og:") {
            SOURCE_OG
        } else if key.starts_with("twitter:") {
            SOURCE_TWITTER
        } else {
            continue;
        };
        metadata.entries.push(MetadataEntry {
            source: source.to_owned(),
            key: key.to_owned(),
            content: decode(&content),
        });
    }
    if let Some(title) = element_text(html, "title") {
        let title = decode(&collapse(&title));
        if !title.is_empty() {
            metadata.title = Some(title.clone());
            metadata.entries.push(MetadataEntry {
                source: SOURCE_HTML.to_owned(),
                key: "title".to_owned(),
                content: title,
            });
        }
    }
    for attrs in tags(html, "link") {
        let Some(rel) = attr(&attrs, "rel") else {
            continue;
        };
        if !rel.eq_ignore_ascii_case("canonical") {
            continue;
        }
        if let Some(href) = attr(&attrs, "href") {
            let href = decode(&href);
            metadata.entries.push(MetadataEntry {
                source: SOURCE_HTML.to_owned(),
                key: "canonical".to_owned(),
                content: href.clone(),
            });
            metadata.canonical = Some(href);
        }
    }
    metadata
}

/// 发现页面**声明**的 RSS/Atom/oEmbed 通道（保文档序）。
pub fn discover_channels(html: &str) -> Vec<Channel> {
    let mut channels = Vec::new();
    for attrs in tags(html, "link") {
        let Some(rel) = attr(&attrs, "rel") else {
            continue;
        };
        if !rel.eq_ignore_ascii_case("alternate") {
            continue;
        }
        let Some(kind) = attr(&attrs, "type").and_then(|t| channel_kind(&t)) else {
            continue;
        };
        let Some(href) = attr(&attrs, "href") else {
            continue;
        };
        let href = decode(&href);
        if href.is_empty() {
            continue;
        }
        channels.push(Channel {
            kind,
            href,
            title: attr(&attrs, "title").map(|t| decode(&t)),
        });
    }
    channels
}

fn channel_kind(mime: &str) -> Option<ChannelKind> {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "application/rss+xml" => Some(ChannelKind::Rss),
        "application/atom+xml" => Some(ChannelKind::Atom),
        "application/json+oembed" | "text/xml+oembed" => Some(ChannelKind::OEmbed),
        _ => None,
    }
}

/// 取出每个 `<name ...>` 开标签的属性文本（忽略大小写；不做完整 HTML 解析）。
///
/// **跳过注释与 `script`/`style` 正文**：页面可以在注释或脚本字符串里写一个假的 `<meta>`，
/// 采信它就等于让页面内容影响我们的判定（#117 复审登记的残余风险 2）。
fn tags(html: &str, name: &str) -> Vec<String> {
    let lowered = html.to_ascii_lowercase();
    let ignored = ignored_ranges(&lowered);
    let needle = format!("<{name}");
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel) = lowered[cursor..].find(&needle) {
        let start = cursor + rel;
        let after = start + needle.len();
        cursor = after;
        if ignored.iter().any(|(s, e)| start >= *s && start < *e) {
            continue;
        }
        // 必须是标签名边界（`<meta` 不能匹配 `<metadata`）。
        let boundary = lowered[after..].chars().next();
        if !matches!(boundary, Some(c) if c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '/' || c == '>')
        {
            continue;
        }
        let Some(gt) = tag_end(&lowered, after) else {
            // 未闭合引号：跳过这个标签继续找，不中断整篇。
            continue;
        };
        out.push(html[after..gt].to_owned());
        cursor = gt + 1;
    }
    out
}

/// 找 `from` 之后第一个**不在引号内**的 `>`（引号里的 `>` 不终止标签）。
fn tag_end(lowered: &str, from: usize) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (offset, c) in lowered[from..].char_indices() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '>' => return Some(from + offset),
                _ => {}
            },
        }
    }
    None
}

/// 标记「不是标签文本」的区间（注释、`script`/`style` 正文）。
///
/// **单遍左→右状态机**（#148 复审指出的回归：旧实现把属性值里的 `<script>`、注释里的 `<script>`、
/// 脚本里的 `<!--` 都当成标签，导致整篇被吞、真实元数据变空）。三条规则：
/// 1. 遇到 `<` 先用引号感知的 `tag_end` 找到标签结束，**整个标签跳过**——属性值里的 `<` 因此永远不会被当成标签开头；
/// 2. 注释（`<!--`）整段跳过；进入 `script`/`style` 正文后整段跳到 `</name>`——注释里的 `<script>`、脚本里的 `<!--` 都不再参与配对；
/// 3. 未闭合的引号只跳过当前 `<` 并继续；未闭合的注释/标签/脚本按 HTML 语义吞到文末。
fn ignored_ranges(lowered: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < lowered.len() {
        // 按**字符宽度**前进：`lowered[i..]` 要求 i 在字符边界上（中文/emoji 否则会 panic）。
        if !lowered[i..].starts_with('<') {
            i += lowered[i..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        if lowered[i..].starts_with("<!--") {
            match lowered[i..].find("-->") {
                Some(rel) => {
                    let next = i + rel + 3;
                    out.push((i, next));
                    i = next;
                }
                None => {
                    out.push((i, lowered.len()));
                    break;
                }
            }
            continue;
        }
        let Some(gt) = tag_end(lowered, i + 1) else {
            i += 1;
            continue;
        };
        let name: String = lowered[i + 1..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if name == "script" || name == "style" {
            let body = gt + 1;
            let close = format!("</{name}");
            match lowered[body..].find(&close) {
                Some(rel) => {
                    out.push((body, body + rel));
                    i = body + rel;
                }
                None => {
                    out.push((body, lowered.len()));
                    break;
                }
            }
            continue;
        }
        i = gt + 1;
    }
    out.sort_unstable();
    out
}
/// 取 `<name>...</name>` 的原文（用于 `<title>`）。
fn element_text(html: &str, name: &str) -> Option<String> {
    let lowered = html.to_ascii_lowercase();
    let ignored = ignored_ranges(&lowered);
    let open = format!("<{name}");
    let mut from = 0usize;
    let start = loop {
        let at = lowered[from..].find(&open)? + from;
        if ignored.iter().any(|(s, e)| at >= *s && at < *e) {
            from = at + open.len();
            continue;
        }
        break at;
    };
    let after = start + open.len();
    let gt = lowered[after..].find('>')? + after;
    let close = format!("</{name}");
    let end = lowered[gt..].find(&close)? + gt;
    Some(html[gt + 1..end].to_owned())
}

/// 取属性值（支持单/双引号，也支持无引号；属性名大小写不敏感）。
fn attr(attrs: &str, name: &str) -> Option<String> {
    let lowered = attrs.to_ascii_lowercase();
    let mut cursor = 0usize;
    while let Some(rel) = lowered[cursor..].find(name) {
        let at = cursor + rel;
        // 属性名左侧必须是分隔符，避免 `data-name` 命中 `name`。
        let prev = attrs[..at].chars().next_back();
        let ok_prev = matches!(
            prev,
            None | Some(' ') | Some('\t') | Some('\n') | Some('\r') | Some('/')
        );
        let after = at + name.len();
        let mut rest = attrs[after..].chars();
        let next = rest.next();
        let ok_next = matches!(next, Some('='));
        let mut padded = false;
        if !ok_next {
            // `name = "v"` 形式
            let mut it = attrs[after..].chars();
            let mut seen = 0usize;
            for c in it.by_ref() {
                if c == ' ' || c == '\t' || c == '\n' || c == '\r' {
                    seen += 1;
                    continue;
                }
                if c == '=' && seen > 0 {
                    padded = true;
                }
                break;
            }
        }
        if ok_prev && (ok_next || padded) {
            let eq = attrs[after..].find('=')? + after;
            let value = attrs[eq + 1..].trim_start();
            let mut chars = value.chars();
            let quote = chars.next()?;
            if quote == '"' || quote == '\'' {
                let end = value[1..].find(quote)? + 1;
                return Some(value[1..end].to_owned());
            }
            // 无引号值：到空白、`>` 或 `/>` 为止。**不能**见到 `/` 就停——
            // 否则 `content=https://example.test/p` 会被截成 `https:`。
            let end = value
                .char_indices()
                .find(|(at, c)| {
                    c.is_whitespace()
                        || *c == '>'
                        || (*c == '/' && value[at + 1..].starts_with('>'))
                })
                .map(|(at, _)| at)
                .unwrap_or(value.len());
            return Some(value[..end].to_owned());
        }
        cursor = after;
    }
    None
}

/// 最小实体解码（**单遍**：`&amp;lt;` 解成字面量 `&lt;`，不会二次解成 `<`）。
fn decode(text: &str) -> String {
    const ENTITIES: [(&str, &str); 6] = [
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&apos;", "'"),
    ];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        match ENTITIES.iter().find(|(key, _)| tail.starts_with(key)) {
            Some((key, value)) => {
                out.push_str(value);
                rest = &tail[key.len()..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<!doctype html><html><head>
      <title>  演示   站点 </title>
      <link rel="canonical" href="https://example.test/a">
      <link rel="alternate" type="application/rss+xml" title="RSS" href="/feed.xml">
      <link rel="alternate" type="application/atom+xml" href="https://example.test/atom">
      <link rel="alternate" type="application/json+oembed" href="/oembed?url=x">
      <meta property="og:title" content="OG 标题 &amp; 更多">
      <meta name="twitter:card" content="summary_large_image">
      <meta name="description" content="这不是 og/twitter，必须被忽略">
    </head><body>正文内容</body></html>"#;

    #[test]
    fn metadata_carries_an_explicit_source() {
        let m = site_metadata(PAGE);
        assert_eq!(m.title.as_deref(), Some("演示 站点"), "title 应去空白");
        assert_eq!(m.canonical.as_deref(), Some("https://example.test/a"));
        let og = m
            .entries
            .iter()
            .find(|e| e.key == "og:title")
            .expect("og:title");
        assert_eq!(og.source, SOURCE_OG);
        assert_eq!(og.content, "OG 标题 & 更多", "应做实体解码");
        let tw = m
            .entries
            .iter()
            .find(|e| e.key == "twitter:card")
            .expect("twitter:card");
        assert_eq!(tw.source, SOURCE_TWITTER);
        assert!(
            m.entries.iter().all(|e| e.key != "description"),
            "只认 og:*/twitter:*，普通 description 不进元数据条目"
        );
        assert!(
            m.entries.iter().all(|e| e.content != "正文内容"),
            "正文绝不能出现在元数据里"
        );
    }

    #[test]
    fn channels_are_discovered_in_document_order() {
        let c = discover_channels(PAGE);
        let kinds: Vec<ChannelKind> = c.iter().map(|x| x.kind).collect();
        assert_eq!(
            kinds,
            vec![ChannelKind::Rss, ChannelKind::Atom, ChannelKind::OEmbed]
        );
        assert_eq!(c[0].href, "/feed.xml");
        assert_eq!(c[0].title.as_deref(), Some("RSS"));
    }

    #[test]
    fn empty_page_yields_empty_not_error() {
        let m = site_metadata("<html><body>只有正文</body></html>");
        assert_eq!(m, SiteMetadata::default(), "没有元数据就是空，不是错误");
        assert!(discover_channels("<html></html>").is_empty());
    }

    #[test]
    fn non_alternate_and_unknown_types_are_ignored() {
        let html = r#"<link rel="stylesheet" type="application/rss+xml" href="/x">
                      <link rel="alternate" type="text/plain" href="/y">"#;
        assert!(
            discover_channels(html).is_empty(),
            "只有 rel=alternate + 已知 MIME 才算通道"
        );
    }

    #[test]
    fn tag_boundary_prevents_false_matches() {
        let html = r#"<metadata><meta name="twitter:card" content="summary"></metadata>"#;
        let m = site_metadata(html);
        assert_eq!(m.entries.len(), 1, "`<metadata` 不该被当成 `<meta`");
        assert_eq!(m.entries[0].key, "twitter:card");
    }

    #[test]
    fn fake_meta_in_comments_and_scripts_is_ignored() {
        let html = r#"<html><head>
          <!-- <meta property="og:title" content="注释里的假货"> -->
          <script>var s = '<meta property="og:url" content="脚本里的假货">';</script>
          <style>/* <meta property="twitter:card" content="样式里的假货"> */</style>
          <meta property="og:title" content="真的">
        </head></html>"#;
        let m = site_metadata(html);
        assert_eq!(
            m.entries.len(),
            1,
            "注释/脚本/样式里的 meta 不得被采信：{m:?}"
        );
        assert_eq!(m.entries[0].content, "真的");
    }

    #[test]
    fn angle_bracket_inside_quotes_does_not_split_tag() {
        let html = r#"<meta property="og:title" content="a > b">"#;
        let m = site_metadata(html);
        assert_eq!(m.entries.len(), 1, "引号内的 > 不该终止标签");
        assert_eq!(m.entries[0].content, "a > b");
    }

    /// #148 复审给出的 6 个**误伤**反例：旧实现会把真实元数据静默吞掉（main 能取到 → 变空）。
    #[test]
    fn ignored_ranges_does_not_swallow_real_metadata() {
        let cases = [
            r#"<div data-x="<script>"></div><meta property="og:title" content="real">"#,
            r#"<!-- 用 <script> 加载 --><meta property="og:title" content="real">"#,
            r#"<!-- <script> --><meta property="og:title" content="real"><script>x()</script>"#,
            r#"<style>a{content:'<script>';}</style><meta property="og:title" content="real">"#,
            r#"<script>var re=/<!--/;</script><meta property="og:title" content="real">"#,
            r#"<meta content="oops><meta property="og:url" content="real">"#,
        ];
        for (index, html) in cases.iter().enumerate() {
            let m = site_metadata(html);
            assert!(
                !m.entries.is_empty(),
                "反例 {index}：真实元数据被误伤吞掉了：{m:?}"
            );
        }
    }

    /// 注释里的假 `<title>` 不得被采信。
    #[test]
    fn fake_title_in_comment_is_ignored() {
        let html = r#"<!-- <title>假标题</title> --><title>真标题</title>"#;
        assert_eq!(site_metadata(html).title.as_deref(), Some("真标题"));
    }

    #[test]
    fn entities_are_decoded_exactly_once() {
        let html = r#"<meta property="og:title" content="&amp;lt;b&amp;gt;">"#;
        let m = site_metadata(html);
        assert_eq!(
            m.entries[0].content, "&lt;b&gt;",
            "只解一次：&amp;lt; 应变成字面量 &lt;，不能二次解成 <"
        );
    }

    #[test]
    fn attribute_values_tolerate_quoting_styles() {
        let html = r#"<meta property='og:url' content=https://example.test/p>"#;
        let m = site_metadata(html);
        assert_eq!(m.entries[0].content, "https://example.test/p");
    }
}
