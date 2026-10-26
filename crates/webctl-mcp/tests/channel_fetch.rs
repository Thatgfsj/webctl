//! #117 第 2 步：`embedded_data` 可选取回站点声明的 RSS/Atom（验收：离线 fixture 各 1 例、
//! 不得把元数据冒充正文、robots 不允许就不拉、全失败要显式降级）。

mod common;

use common::http_fixture;
use common::in_process_context;
use serde_json::{Value, json};
use webctl_mcp::WebctlMcp;

const PAGE: &str = r#"<html><head><title>页面标题</title>
  <link rel="alternate" type="application/rss+xml" title="RSS" href="/feed.xml">
  <meta property="og:title" content="OG 标题">
</head><body><p>正文</p></body></html>"#;

const RSS: &str = r#"<?xml version="1.0"?><rss version="2.0"><channel>
  <title>Example Feed</title>
  <item><title>First</title><link>https://example.test/a</link></item>
  <item><title>Second</title><link>https://example.test/b</link></item>
</channel></rss>"#;

const ATOM: &str = r#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom Feed</title>
  <entry><title>Only entry</title><link rel="alternate" href="https://example.test/1"/></entry>
</feed>"#;

async fn invoke(mcp: &WebctlMcp, name: &str, args: Value) -> Value {
    mcp.invoke(name, args)
        .await
        .unwrap_or_else(|| panic!("工具 {name} 未注册"))
}

fn feed_url(envelope: &Value) -> String {
    envelope["data"]["feed_channels"][0]["url"]
        .as_str()
        .expect("feed url")
        .to_owned()
}

/// 默认关：不声明 `fetch_channels` 时**一个 feed 请求都不发**。
#[tokio::test]
async fn feed_channels_are_off_by_default() {
    let server = http_fixture::Server::start_with(vec![
        ("/page", http_fixture::Response::html(PAGE)),
        ("/feed.xml", http_fixture::Response::xml(RSS)),
    ])
    .await;
    let mcp = WebctlMcp::new(in_process_context("feed-off"));
    let envelope = invoke(&mcp, "embedded_data", json!({"url": server.url("/page")})).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert!(
        envelope["data"]["feed_channels"].is_null(),
        "默认不返回 feed_channels：{envelope}"
    );
    assert_eq!(server.count("/feed.xml"), 0, "默认不得请求 feed");
}

/// 显式 opt-in：取回 RSS 并解析出条目（与 metadata 分开，source/kinds 明确）。
#[tokio::test]
async fn feed_channels_opt_in_parses_rss() {
    let server = http_fixture::Server::start_with(vec![
        ("/page", http_fixture::Response::html(PAGE)),
        ("/feed.xml", http_fixture::Response::xml(RSS)),
    ])
    .await;
    let mcp = WebctlMcp::new(in_process_context("feed-on"));
    let envelope = invoke(
        &mcp,
        "embedded_data",
        json!({"url": server.url("/page"), "fetch_channels": true}),
    )
    .await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    let channel = &envelope["data"]["feed_channels"][0];
    assert_eq!(channel["kind"], "rss");
    assert_eq!(channel["status"], "hit");
    assert_eq!(channel["title"], "Example Feed");
    assert_eq!(channel["items"].as_array().expect("items").len(), 2);
    assert_eq!(channel["items"][0]["title"], "First");
    assert!(
        feed_url(&envelope).ends_with("/feed.xml"),
        "url 应是解析后的绝对地址：{envelope}"
    );
    // 元数据与正文各自独立：og:title 只出现在 metadata，不在 feed 条目里。
    assert_eq!(envelope["data"]["metadata"]["title"], "页面标题");
    // 验收：元数据必须与正文/条目分开——og:title 只是 entries 里的 og-meta 一条。
    let metadata_entries = envelope["data"]["metadata"]["entries"]
        .as_array()
        .expect("metadata entries");
    assert!(
        metadata_entries
            .iter()
            .any(|entry| entry["key"] == "og:title"
                && entry["source"] == "og-meta"
                && entry["content"] == "OG 标题"),
        "og:title 应作为 og-meta 条目出现：{envelope}"
    );
    assert_eq!(server.count("/feed.xml"), 1);
}

/// Atom 也支持（link 用 href）。
#[tokio::test]
async fn feed_channels_opt_in_parses_atom() {
    let page = PAGE.replace("application/rss+xml", "application/atom+xml");
    let server = http_fixture::Server::start_with(vec![
        ("/page", http_fixture::Response::html(page)),
        ("/feed.xml", http_fixture::Response::xml(ATOM)),
    ])
    .await;
    let mcp = WebctlMcp::new(in_process_context("feed-atom"));
    let envelope = invoke(
        &mcp,
        "embedded_data",
        json!({"url": server.url("/page"), "fetch_channels": true}),
    )
    .await;
    let channel = &envelope["data"]["feed_channels"][0];
    assert_eq!(channel["kind"], "atom");
    assert_eq!(channel["status"], "hit");
    assert_eq!(channel["items"][0]["link"], "https://example.test/1");
}

/// robots 不允许：**不请求** feed，逐通道如实报 blocked，并显式降级。
#[tokio::test]
async fn robots_disallow_blocks_feed_without_request() {
    let server = http_fixture::Server::start_with(vec![
        ("/page", http_fixture::Response::html(PAGE)),
        ("/feed.xml", http_fixture::Response::xml(RSS)),
        (
            "/robots.txt",
            http_fixture::Response::robots("User-agent: *\nDisallow: /feed.xml\n"),
        ),
    ])
    .await;
    let mcp = WebctlMcp::new(in_process_context("feed-robots"));
    let envelope = invoke(
        &mcp,
        "embedded_data",
        json!({"url": server.url("/page"), "fetch_channels": true}),
    )
    .await;
    assert_eq!(server.count("/feed.xml"), 0, "robots 禁止时不得请求 feed");
    let channel = &envelope["data"]["feed_channels"][0];
    assert_eq!(channel["status"], "blocked");
    assert!(
        channel["note"]
            .as_str()
            .is_some_and(|note| !note.is_empty()),
        "被拦下要保留原因：{envelope}"
    );
    assert_eq!(
        envelope["code"], "DEGRADED",
        "全失败必须显式降级：{envelope}"
    );
    assert_eq!(envelope["meta"]["degraded_from"], "channel-fetch-failed");
    assert_eq!(envelope["meta"]["actionable"]["reason"], "feed-unavailable");
}

/// 全失败（404）同样显式降级，不静默返回 OK + 空。
#[tokio::test]
async fn all_channels_failing_degrades_explicitly() {
    let server = http_fixture::Server::start_with(vec![
        ("/page", http_fixture::Response::html(PAGE)),
        ("/feed.xml", http_fixture::Response::status(404)),
    ])
    .await;
    let mcp = WebctlMcp::new(in_process_context("feed-404"));
    let envelope = invoke(
        &mcp,
        "embedded_data",
        json!({"url": server.url("/page"), "fetch_channels": true}),
    )
    .await;
    assert_eq!(envelope["code"], "DEGRADED", "{envelope}");
    assert_eq!(envelope["meta"]["degraded_from"], "channel-fetch-failed");
    assert_eq!(
        envelope["data"]["feed_channels"][0]["status"],
        "unavailable"
    );
}
