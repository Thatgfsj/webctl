//! 站点级通道的**取回**（议题 #117 第 2 步 / #196 第 3 步）。
//!
//! 边界：
//! - rss / atom：取回并解析条目（#117 第 2 步）；
//! - oEmbed：对页面声明的端点做 JSON 拉取，只保留白名单字段；HTML 型端点只取
//!   <title>/摘要并标注来源（#196）；
//! - sitemap：显式 opt-in 时解析 sitemap.xml / sitemap index 产出 URL 列表，
//!   **只列 URL、不抓取这些页面**；robots.txt 声明的 Sitemap: 优先（#196）；
//! - 每次最多尝试 MAX_CHANNELS 个通道（含 oEmbed）、每个 feed 最多 MAX_ITEMS 条；
//! - 全部走 PoliteFetcher：robots 不允许就**不请求**（status=blocked）；
//! - 结果都是"线索"而非正文，调用方必须按 kind/source 区分。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::content_type::ContentKind;
use crate::parse::sitemap::{
    MAX_SITEMAP_CHILDREN, MAX_SITEMAP_DEPTH, MAX_SITEMAP_ENTRIES, SitemapKind, parse_sitemap,
};
use crate::parse::{Channel, ChannelKind, FeedItem, parse_feed, site_metadata};
use crate::politeness::PoliteFetcher;

/// 每次最多尝试几个通道（rss/atom/oembed 合计；与 resolve_site 的 L2/L4 同一预算思路）。
pub const MAX_CHANNELS: usize = 2;

/// 每个 feed 通道最多返回几条。
pub const MAX_ITEMS: usize = 10;

/// oEmbed 白名单字段：只收标量，**不收** html 嵌入代码或未登记字段。
pub const OEMBED_FIELDS: [&str; 13] = [
    "type",
    "version",
    "title",
    "author_name",
    "author_url",
    "provider_name",
    "provider_url",
    "thumbnail_url",
    "thumbnail_width",
    "thumbnail_height",
    "width",
    "height",
    "cache_age",
];

/// 单通道取回状态（逐通道如实报告，不聚合成"没找到"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelStatus {
    /// 取回并解析出至少一条 / 至少一个白名单字段。
    Hit,
    /// 取回成功但没有任何可用内容。
    Empty,
    /// 取不到（网络/404/非文本/解析不了）。
    Unavailable,
    /// robots 不允许（**未发请求**）。
    Blocked,
    /// 未尝试（保留的兼容状态；预算用尽时通道不会出现在结果里）。
    Skipped,
}

/// oEmbed 端点的白名单结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OEmbedInfo {
    /// 来源：oembed-json（JSON 端点）或 oembed-html（HTML 端点，只取标题/摘要）。
    pub source: String,
    /// 白名单字段（键排序；值统一为字符串，数字原样转写）。
    pub fields: BTreeMap<String, String>,
    /// HTML 型端点的摘要（og:description / twitter:description）；JSON 型恒为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// 一个通道的取回结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FetchedChannel {
    /// 通道类型。
    pub kind: ChannelKind,
    /// 页面上声明的 href（原样）。
    pub declared_href: String,
    /// 相对页面 URL 解析后的绝对地址（解析失败为 None）。
    pub url: Option<String>,
    /// 状态。
    pub status: ChannelStatus,
    /// 标题（feed 标题或 oEmbed 的 title）。
    pub title: Option<String>,
    /// 条目（最多 MAX_ITEMS 条；oEmbed 恒为空）。
    pub items: Vec<FeedItem>,
    /// oEmbed 白名单结果（非 oEmbed 通道为 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oembed: Option<OEmbedInfo>,
    /// 可读说明（被机器人拦下时保留原因）。
    pub note: String,
}

/// 依次取回声明里可处理的通道（rss/atom/oembed），最多 MAX_CHANNELS 个。
pub async fn fetch_channels(
    page_url: &Url,
    channels: &[Channel],
    fetcher: &PoliteFetcher,
) -> Vec<FetchedChannel> {
    let mut out = Vec::new();
    for channel in channels.iter().take(MAX_CHANNELS) {
        out.push(fetch_one(page_url, channel, fetcher).await);
    }
    out
}

async fn fetch_one(page_url: &Url, channel: &Channel, fetcher: &PoliteFetcher) -> FetchedChannel {
    let base = FetchedChannel {
        kind: channel.kind,
        declared_href: channel.href.clone(),
        url: None,
        status: ChannelStatus::Unavailable,
        title: None,
        items: Vec::new(),
        oembed: None,
        note: String::new(),
    };
    let Ok(url) = page_url.join(&channel.href) else {
        return FetchedChannel {
            note: "通道地址无法相对页面 URL 解析".to_owned(),
            ..base
        };
    };
    if !matches!(url.scheme(), "http" | "https") {
        return FetchedChannel {
            note: format!("不支持的通道协议：{}", url.scheme()),
            ..base
        };
    }
    let mut out = FetchedChannel {
        url: Some(url.to_string()),
        ..base
    };
    match channel.kind {
        ChannelKind::OEmbed => fetch_oembed(&mut out, &url, fetcher).await,
        ChannelKind::Rss | ChannelKind::Atom => fetch_feed(&mut out, &url, fetcher).await,
    }
    out
}

/// 按**原因**区分失败：只有 robots 拦截才是"被站点策略拒绝"（未发请求）。
fn failure_status(code: webctl_core::ErrorCode, error: &str) -> (ChannelStatus, String) {
    let blocked = code == webctl_core::ErrorCode::Blocked && error.contains("robots");
    let note = if error.is_empty() {
        format!("取回失败（{}）", code.as_str())
    } else {
        error.to_owned()
    };
    let status = if blocked {
        ChannelStatus::Blocked
    } else {
        ChannelStatus::Unavailable
    };
    (status, note)
}

async fn fetch_feed(out: &mut FetchedChannel, url: &Url, fetcher: &PoliteFetcher) {
    let envelope = fetcher.fetch(url.as_str()).await;
    if !envelope.ok {
        let (status, note) = failure_status(envelope.code, &envelope.error);
        out.status = status;
        out.note = note;
        return;
    }
    let Some(fetched) = envelope.data else {
        out.note = "取回成功但没有内容".to_owned();
        return;
    };
    let Some(text) = fetched.fetched.content.text.as_deref() else {
        out.note = "feed 响应不是文本（按不可用处理）".to_owned();
        return;
    };
    let report = parse_feed(text);
    out.title = report.title;
    if report.items.is_empty() {
        out.status = ChannelStatus::Empty;
        out.note = report.note;
        return;
    }
    let total = report.items.len();
    let mut items = report.items;
    if items.len() > MAX_ITEMS {
        items.truncate(MAX_ITEMS);
    }
    out.status = ChannelStatus::Hit;
    out.note = format!("{} 条（返回前 {MAX_ITEMS} 条，共 {total} 条）", items.len());
    out.items = items;
}

async fn fetch_oembed(out: &mut FetchedChannel, url: &Url, fetcher: &PoliteFetcher) {
    let envelope = fetcher.fetch(url.as_str()).await;
    if !envelope.ok {
        let (status, note) = failure_status(envelope.code, &envelope.error);
        out.status = status;
        out.note = note;
        return;
    }
    let Some(fetched) = envelope.data else {
        out.note = "取回成功但没有内容".to_owned();
        return;
    };
    let kind = fetched.fetched.content.kind;
    let Some(text) = fetched.fetched.content.text.as_deref() else {
        out.note = "oEmbed 响应不是文本（按不可用处理）".to_owned();
        return;
    };
    match kind {
        ContentKind::Json => {
            let value: serde_json::Value = match serde_json::from_str(text) {
                Ok(value) => value,
                Err(error) => {
                    out.status = ChannelStatus::Unavailable;
                    out.note = format!("oEmbed 响应不是合法 JSON：{error}");
                    return;
                }
            };
            let info = oembed_from_json(&value);
            out.title = info.fields.get("title").cloned();
            if info.fields.is_empty() {
                out.status = ChannelStatus::Empty;
                out.note = "oEmbed JSON 里没有白名单字段（可能不是 oEmbed 端点）".to_owned();
            } else {
                out.status = ChannelStatus::Hit;
                out.note = format!(
                    "oEmbed JSON，{} 个白名单字段（其余字段已丢弃）",
                    info.fields.len()
                );
            }
            out.oembed = Some(info);
        }
        ContentKind::Html | ContentKind::Xml | ContentKind::Text => {
            // HTML 型端点只取标题/摘要；正文一律不采信（#196）。
            let metadata = site_metadata(text);
            let summary = metadata
                .entries
                .iter()
                .find(|entry| entry.key == "og:description" || entry.key == "twitter:description")
                .map(|entry| entry.content.clone());
            out.title = metadata.title;
            if out.title.is_none() && summary.is_none() {
                out.status = ChannelStatus::Empty;
                out.note = "HTML 型 oEmbed 里没有 <title> 或摘要".to_owned();
            } else {
                out.status = ChannelStatus::Hit;
                out.note = "HTML 型 oEmbed：只取 <title>/摘要，正文不采信".to_owned();
            }
            out.oembed = Some(OEmbedInfo {
                source: "oembed-html".to_owned(),
                fields: BTreeMap::new(),
                summary,
            });
        }
        _ => {
            out.status = ChannelStatus::Unavailable;
            out.note = format!("oEmbed 响应类型不可处理（{}）", kind.as_str());
        }
    }
}

/// 只把白名单内的**标量**字段收进来；对象/数组/未登记键一律丢弃。
fn oembed_from_json(value: &serde_json::Value) -> OEmbedInfo {
    let mut fields = BTreeMap::new();
    if let Some(object) = value.as_object() {
        for key in OEMBED_FIELDS {
            let Some(raw) = object.get(key) else {
                continue;
            };
            let text = match raw {
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Number(number) => number.to_string(),
                serde_json::Value::Bool(flag) => flag.to_string(),
                _ => continue,
            };
            if !text.is_empty() {
                fields.insert(key.to_owned(), text);
            }
        }
    }
    OEmbedInfo {
        source: "oembed-json".to_owned(),
        fields,
        summary: None,
    }
}

/// sitemap 条目发现的结果（**只列 URL，不抓取这些 URL**）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchedSitemap {
    /// 入口来源：robots（robots.txt 声明的 Sitemap: 优先）或 well-known（/sitemap.xml）。
    pub source: String,
    /// 状态：hit / empty / unavailable / blocked。
    pub status: ChannelStatus,
    /// 实际取回并解析的 sitemap 文档（最多 MAX_SITEMAP_CHILDREN 个）。
    pub documents: Vec<String>,
    /// 发现到的页面 URL（去重、保序、最多 MAX_SITEMAP_ENTRIES 条）。
    pub urls: Vec<String>,
    /// 是否因条目/文档/递归上限被截断。
    pub truncated: bool,
    /// 可读说明。
    pub note: String,
}

impl FetchedSitemap {
    /// 未尝试/无法定位时的结果（例如 html 输入没有基准 URL）。
    pub fn unavailable(note: impl Into<String>) -> Self {
        Self {
            source: "none".to_owned(),
            status: ChannelStatus::Unavailable,
            documents: Vec::new(),
            urls: Vec::new(),
            truncated: false,
            note: note.into(),
        }
    }
}

/// 显式 opt-in 的 sitemap 条目发现：robots 声明优先，否则尝试 origin 的 /sitemap.xml。
///
/// 递归上限 MAX_SITEMAP_DEPTH、文档上限 MAX_SITEMAP_CHILDREN、条目上限 MAX_SITEMAP_ENTRIES；
/// 命中上限时 truncated=true。robots 不允许的文档**不请求**并计入 blocked。
pub async fn fetch_sitemap(page_url: &Url, fetcher: &PoliteFetcher) -> FetchedSitemap {
    let declared = fetcher
        .robots_rules(page_url)
        .await
        .map(|rules| rules.sitemaps().to_vec())
        .unwrap_or_default();
    let (source, mut queue): (String, Vec<(String, usize)>) = if declared.is_empty() {
        (
            "well-known".to_owned(),
            vec![(
                format!("{}/sitemap.xml", page_url.origin().ascii_serialization()),
                0,
            )],
        )
    } else {
        (
            "robots".to_owned(),
            declared
                .into_iter()
                .take(MAX_SITEMAP_CHILDREN)
                .map(|url| (url, 0))
                .collect(),
        )
    };

    let mut documents: Vec<String> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut notes: Vec<String> = Vec::new();
    let mut truncated = false;
    let mut blocked = false;
    let mut cursor = 0usize;
    while cursor < queue.len() {
        if documents.len() >= MAX_SITEMAP_CHILDREN || urls.len() >= MAX_SITEMAP_ENTRIES {
            truncated = true;
            break;
        }
        let (target, depth) = queue[cursor].clone();
        cursor += 1;
        let envelope = fetcher.fetch(&target).await;
        if !envelope.ok {
            let (status, note) = failure_status(envelope.code, &envelope.error);
            if status == ChannelStatus::Blocked {
                blocked = true;
            }
            notes.push(format!("{target}: {note}"));
            continue;
        }
        let Some(fetched) = envelope.data else {
            notes.push(format!("{target}: 取回成功但没有内容"));
            continue;
        };
        let Some(text) = fetched.fetched.content.text.as_deref() else {
            notes.push(format!("{target}: 不是文本（按不可用处理）"));
            continue;
        };
        let doc = parse_sitemap(text);
        documents.push(target.clone());
        if doc.truncated {
            truncated = true;
        }
        match doc.kind {
            SitemapKind::UrlSet => {
                for loc in doc.locs {
                    if urls.len() >= MAX_SITEMAP_ENTRIES {
                        truncated = true;
                        break;
                    }
                    if seen.insert(loc.clone()) {
                        urls.push(loc);
                    }
                }
            }
            SitemapKind::Index => {
                if depth + 1 >= MAX_SITEMAP_DEPTH {
                    truncated = true;
                    notes.push(format!("{target}: 已达递归层数上限，子 sitemap 未展开"));
                } else {
                    for child in doc.locs {
                        let pending = documents.len() + (queue.len() - cursor);
                        if pending >= MAX_SITEMAP_CHILDREN {
                            truncated = true;
                            break;
                        }
                        queue.push((child, depth + 1));
                    }
                }
            }
            SitemapKind::Unknown => notes.push(format!("{target}: {}", doc.note)),
        }
    }

    let status = if !urls.is_empty() {
        ChannelStatus::Hit
    } else if !documents.is_empty() {
        ChannelStatus::Empty
    } else if blocked {
        ChannelStatus::Blocked
    } else {
        ChannelStatus::Unavailable
    };
    let summary = format!(
        "{source} 入口，{} 个文档，{} 条 URL",
        documents.len(),
        urls.len()
    );
    let note = if notes.is_empty() {
        summary
    } else {
        format!("{summary}；{}", notes.join("；"))
    };
    FetchedSitemap {
        source,
        status,
        documents,
        urls,
        truncated,
        note,
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// oEmbed 只收白名单内的标量：`html` 嵌入代码与未登记字段不得出现；全缺失时是空结果而不是猜一个。
    #[test]
    fn oembed_fields_are_whitelisted_and_empty_is_empty() {
        let info = oembed_from_json(&serde_json::json!({
            "type": "video",
            "title": "标题",
            "thumbnail_width": 480,
            "html": "<iframe></iframe>",
            "secret": "must-not-leak"
        }));
        assert_eq!(info.source, "oembed-json");
        assert_eq!(info.fields.get("title").map(String::as_str), Some("标题"));
        assert_eq!(
            info.fields.get("thumbnail_width").map(String::as_str),
            Some("480")
        );
        assert!(!info.fields.contains_key("html"));
        assert!(!info.fields.contains_key("secret"));
        assert!(
            oembed_from_json(&serde_json::json!({"foo": "bar"}))
                .fields
                .is_empty()
        );
    }
}
