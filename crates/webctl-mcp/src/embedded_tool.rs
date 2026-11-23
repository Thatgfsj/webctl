//! embedded_data 工具（#114 内嵌结构化数据 / #117 第 2 步 / #196 第 3 步）。
//!
//! 单独成模块：dispatch.rs 已接近 1000 行上限（仓库硬规则）。

use serde_json::Value;
use url::Url;

use webctl_core::{Envelope, ErrorCode};
use webctl_web::parse::{
    ApiDiscoveryOptions, EmbeddedOptions, discover_api_endpoints, extract_embedded,
};
use webctl_web::politeness::{PoliteFetcher, PolitenessConfig};

use crate::context::McpContext;
use crate::params::EmbeddedDataParams;

use crate::dispatch::{apply_render_degradation, parse, resolve_source, to_value};

/// embedded_data：抽取页面内嵌的结构化数据。
///
/// 零 JS 执行：只解析 HTML 里已经存在的 JSON 载荷（JSON-LD / script[type=json] /
/// 内联状态 / data-page）。语义与 webctl_web::parse::extract_embedded 一致：
/// 零结果是合法的空结果，容器在但全部解析失败才是 PARSE。
pub(crate) async fn tool_embedded_data(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<EmbeddedDataParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    let source = match resolve_source(ctx, params.url.as_deref(), params.html.as_deref()).await {
        Ok(source) => source,
        Err(value) => return value,
    };
    let mut options = EmbeddedOptions::default();
    if let Some(value) = params.max_entries {
        options.max_entries = value;
    }
    if let Some(value) = params.max_entry_bytes {
        options.max_entry_bytes = value;
    }
    if let Some(value) = params.include_inline_state {
        options.include_inline_state = value;
    }
    let mut envelope = extract_embedded(&source.html, &options);

    // #117 第 2 步 / #196：可选取回页面声明的通道（默认关，显式 opt-in；一律过 politeness）。
    let mut channel_failed = false;
    if params.fetch_channels.unwrap_or(false)
        && let Some(report) = envelope.data.as_mut()
    {
        let channels = report.channels.clone();
        let fetched = if channels.is_empty() {
            Vec::new()
        } else if let Some(url) = params.url.as_deref().and_then(|url| Url::parse(url).ok()) {
            let fetcher = PoliteFetcher::new(PolitenessConfig::default(), ctx.fetch_options());
            webctl_web::channels::fetch_channels(&url, &channels, &fetcher).await
        } else {
            // html 输入没有基准 URL：相对地址无法解析，如实标注而不是猜。
            channels
                .iter()
                .map(|channel| webctl_web::channels::FetchedChannel {
                    kind: channel.kind,
                    declared_href: channel.href.clone(),
                    url: None,
                    status: webctl_web::channels::ChannelStatus::Unavailable,
                    title: None,
                    items: Vec::new(),
                    oembed: None,
                    note: "html 输入没有基准 URL，无法解析通道地址".to_owned(),
                })
                .collect()
        };
        let attempted = fetched
            .iter()
            .filter(|channel| {
                !matches!(channel.status, webctl_web::channels::ChannelStatus::Skipped)
            })
            .count();
        let hits = fetched
            .iter()
            .filter(|channel| matches!(channel.status, webctl_web::channels::ChannelStatus::Hit))
            .count();
        channel_failed = attempted > 0 && hits == 0;
        report.feed_channels = Some(fetched);
    }

    // #196 第 3 步：sitemap 条目发现（默认关；只列 URL，不抓取这些页面）。
    let mut sitemap_failed = false;
    if params.fetch_sitemap.unwrap_or(false)
        && let Some(report) = envelope.data.as_mut()
    {
        let sitemap = match params.url.as_deref().and_then(|url| Url::parse(url).ok()) {
            Some(url) => {
                let fetcher = PoliteFetcher::new(PolitenessConfig::default(), ctx.fetch_options());
                webctl_web::channels::fetch_sitemap(&url, &fetcher).await
            }
            None => webctl_web::channels::FetchedSitemap::unavailable(
                "html 输入没有基准 URL，无法定位 sitemap",
            ),
        };
        sitemap_failed = matches!(
            sitemap.status,
            webctl_web::channels::ChannelStatus::Unavailable
                | webctl_web::channels::ChannelStatus::Blocked
        );
        report.sitemap = Some(sitemap);
    }

    if channel_failed || sitemap_failed {
        let from = match (channel_failed, sitemap_failed) {
            (true, true) => "channel-fetch-failed,sitemap-fetch-failed",
            (true, false) => "channel-fetch-failed",
            (false, true) => "sitemap-fetch-failed",
            (false, false) => unreachable!("外层条件已排除"),
        };
        let elapsed = envelope.meta.elapsed_ms;
        let report = envelope.data.expect("上方已确认 data 存在");
        // 全失败：显式降级并带可行动信息（原因码表见 webctl_core::actionable）。
        envelope = Envelope::degraded(report, from, ErrorCode::Degraded, elapsed);
    }

    // #116：API 端点线索。纯函数、零网络：只报告页面源码里明写的地址，绝不探测。
    // 基准优先取显式 base，其次取页面最终 URL（重定向后）；html 输入没有基准时相对路径进 unresolved。
    let base = params
        .base
        .as_deref()
        .and_then(|raw| Url::parse(raw).ok())
        .or_else(|| source.base_url.clone());
    let api_candidates =
        discover_api_endpoints(&source.html, base.as_ref(), &ApiDiscoveryOptions::default());
    let mut value = to_value(envelope);
    if let Some(data) = value.get_mut("data").and_then(Value::as_object_mut) {
        data.insert(
            "api_candidates".to_owned(),
            serde_json::to_value(api_candidates).unwrap_or(Value::Null),
        );
    }
    apply_render_degradation(value, source.render.as_ref())
}
