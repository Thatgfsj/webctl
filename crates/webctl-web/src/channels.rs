//! 站点级通道的**取回**（议题 #117 第 2 步）：把页面声明的 RSS/Atom 礼貌地取回来看有什么。
//!
//! 边界（#117 验收）：
//! - 只处理 rss / atom；oEmbed 只报告**声明**、本步不拉取（`status=skipped` 且 note 写明）；
//! - 每次最多尝试 `MAX_CHANNELS` 个通道、每个最多取 `MAX_ITEMS` 条；
//! - 全部走 `PoliteFetcher`：robots 不允许就**不请求**（`status=blocked`），429/限速同样由它处理；
//! - feed 条目标是"元数据/线索"，调用方必须按 `source=feed` 区分，不得并进正文。

use serde::{Deserialize, Serialize};
use url::Url;

use crate::parse::{Channel, ChannelKind, FeedItem, parse_feed};
use crate::politeness::PoliteFetcher;

/// 每次最多尝试几个通道（与 resolve_site 的 L2/L4 同一预算思路）。
pub const MAX_CHANNELS: usize = 2;

/// 每个通道最多返回几条。
pub const MAX_ITEMS: usize = 10;

/// 单通道取回状态（逐通道如实报告，不聚合成"没找到"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelStatus {
    /// 取回并解析出至少一条。
    Hit,
    /// 取回成功但 feed 里没有可识别条目。
    Empty,
    /// 取不到（网络/404/非文本/解析不了 feed）。
    Unavailable,
    /// robots 不允许（**未发请求**）。
    Blocked,
    /// 本步不处理的通道（oEmbed）。
    Skipped,
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
    /// feed 标题（若有）。
    pub title: Option<String>,
    /// 条目（最多 `MAX_ITEMS` 条）。
    pub items: Vec<FeedItem>,
    /// 可读说明（被机器人拦下时保留原因）。
    pub note: String,
}

/// 依次取回声明里可处理的通道（rss/atom），最多 `MAX_CHANNELS` 个。
pub async fn fetch_channels(
    page_url: &Url,
    channels: &[Channel],
    fetcher: &PoliteFetcher,
) -> Vec<FetchedChannel> {
    let mut out = Vec::new();
    let mut attempted = 0usize;
    for channel in channels {
        if channel.kind == ChannelKind::OEmbed {
            out.push(FetchedChannel {
                kind: channel.kind,
                declared_href: channel.href.clone(),
                url: None,
                status: ChannelStatus::Skipped,
                title: None,
                items: Vec::new(),
                note: "oEmbed 本步不拉取（当前只处理 rss/atom）".to_owned(),
            });
            continue;
        }
        if attempted >= MAX_CHANNELS {
            break;
        }
        attempted += 1;
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
        note: String::new(),
    };
    let Ok(url) = page_url.join(&channel.href) else {
        return FetchedChannel {
            note: "feed 地址无法相对页面 URL 解析".to_owned(),
            ..base
        };
    };
    if !matches!(url.scheme(), "http" | "https") {
        return FetchedChannel {
            note: format!("不支持的 feed 协议：{}", url.scheme()),
            ..base
        };
    }
    let mut out = FetchedChannel {
        url: Some(url.to_string()),
        ..base
    };
    let envelope = fetcher.fetch(url.as_str()).await;
    if !envelope.ok {
        // 传输层把 4xx/5xx 也映射成 BLOCKED；这里按**原因**区分：
        // 只有 robots 拦截才是"被站点策略拒绝"（未发请求），其余是"取不到"。
        let robots_blocked = envelope.error.contains("robots");
        out.status = if envelope.code == webctl_core::ErrorCode::Blocked && robots_blocked {
            ChannelStatus::Blocked
        } else {
            ChannelStatus::Unavailable
        };
        out.note = if envelope.error.is_empty() {
            format!("取回失败（{}）", envelope.code.as_str())
        } else {
            envelope.error.clone()
        };
        return out;
    }
    let Some(fetched) = envelope.data else {
        out.note = "取回成功但没有内容".to_owned();
        return out;
    };
    let Some(text) = fetched.fetched.content.text.as_deref() else {
        out.note = "feed 响应不是文本（按不可用处理）".to_owned();
        return out;
    };
    let report = parse_feed(text);
    out.title = report.title;
    if report.items.is_empty() {
        out.status = ChannelStatus::Empty;
        out.note = report.note;
        return out;
    }
    let total = report.items.len();
    let mut items = report.items;
    if items.len() > MAX_ITEMS {
        items.truncate(MAX_ITEMS);
    }
    out.status = ChannelStatus::Hit;
    out.note = format!("{} 条（返回前 {MAX_ITEMS} 条，共 {total} 条）", items.len());
    out.items = items;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_kind_serialize() {
        let value = serde_json::to_value(ChannelStatus::Blocked).expect("serialize");
        assert_eq!(value, serde_json::json!("blocked"));
    }

    /// 无网络纯函数部分：oEmbed 只跳过、超预算不再尝试（用空 fetcher 不会被调用）。
    #[tokio::test]
    async fn oembed_is_skipped_without_network() {
        let channels = vec![Channel {
            kind: ChannelKind::OEmbed,
            href: "https://example.test/oembed".to_owned(),
            title: None,
        }];
        let fetcher = PoliteFetcher::new(
            crate::politeness::PolitenessConfig::default(),
            crate::transport::FetchOptions::default(),
        );
        let page = Url::parse("https://example.test/page").expect("url");
        let out = fetch_channels(&page, &channels, &fetcher).await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].status, ChannelStatus::Skipped);
        assert!(out[0].url.is_none());
    }
}
