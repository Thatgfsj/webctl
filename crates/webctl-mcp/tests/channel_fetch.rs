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

const OEMBED_PAGE: &str = r#"<html><head><title>页面标题</title>
  <link rel="alternate" type="application/json+oembed" href="/oembed.json">
  <link rel="alternate" type="text/xml+oembed" href="/oembed-html">
</head><body><p>正文</p></body></html>"#;

const OEMBED_JSON: &str = r#"{"type":"video","version":"1.0","title":"视频标题",
  "author_name":"作者","thumbnail_width":480,
  "html":"<iframe></iframe>","secret":"must-not-leak"}"#;

const OEMBED_HTML: &str = r#"<html><head><title>嵌入页标题</title>
  <meta property="og:description" content="摘要一句">
</head><body>不应采信的正文</body></html>"#;

const SITEMAP_URLSET: &str = r#"<urlset>
  <url><loc>https://example.test/a</loc></url>
  <url><loc>https://example.test/b</loc></url>
</urlset>"#;

/// oEmbed：JSON 只留白名单标量（`html`/未登记字段丢弃），HTML 型只取标题/摘要、正文不采信。
#[tokio::test]
async fn oembed_is_limited_to_whitelisted_fields() {
    let server = http_fixture::Server::start_with(vec![
        ("/page", http_fixture::Response::html(OEMBED_PAGE)),
        ("/oembed.json", http_fixture::Response::json(OEMBED_JSON)),
        ("/oembed-html", http_fixture::Response::html(OEMBED_HTML)),
    ])
    .await;
    let mcp = WebctlMcp::new(in_process_context("oembed-json"));
    let envelope = invoke(
        &mcp,
        "embedded_data",
        json!({"url": server.url("/page"), "fetch_channels": true}),
    )
    .await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    let json = &envelope["data"]["feed_channels"][0];
    assert_eq!(json["status"], "hit");
    assert_eq!(json["oembed"]["source"], "oembed-json");
    assert_eq!(json["oembed"]["fields"]["title"], "视频标题");
    assert_eq!(json["oembed"]["fields"]["thumbnail_width"], "480");
    assert!(
        json["oembed"]["fields"].get("html").is_none(),
        "白名单外的 html 不得出现：{envelope}"
    );
    assert!(
        json["oembed"]["fields"].get("secret").is_none(),
        "未登记字段不得出现：{envelope}"
    );
    assert_eq!(json["items"].as_array().expect("items").len(), 0);
    let html = &envelope["data"]["feed_channels"][1];
    assert_eq!(html["status"], "hit");
    assert_eq!(html["oembed"]["source"], "oembed-html");
    assert_eq!(html["title"], "嵌入页标题");
    assert_eq!(html["oembed"]["summary"], "摘要一句");
    assert!(
        html["oembed"]["fields"]
            .as_object()
            .expect("fields")
            .is_empty()
    );
    assert!(
        !envelope.to_string().contains("不应采信的正文"),
        "正文绝不能进入结果：{envelope}"
    );
    assert_eq!(server.count("/oembed.json"), 1);
}

/// robots 不允许 oEmbed 端点：不请求，如实报 blocked 并显式降级。
#[tokio::test]
async fn oembed_robots_blocks_without_request() {
    let server = http_fixture::Server::start_with(vec![
        ("/page", http_fixture::Response::html(OEMBED_PAGE)),
        ("/oembed.json", http_fixture::Response::json(OEMBED_JSON)),
        (
            "/robots.txt",
            http_fixture::Response::robots("User-agent: *\nDisallow: /oembed.json\n"),
        ),
    ])
    .await;
    let mcp = WebctlMcp::new(in_process_context("oembed-robots"));
    let envelope = invoke(
        &mcp,
        "embedded_data",
        json!({"url": server.url("/page"), "fetch_channels": true}),
    )
    .await;
    assert_eq!(server.count("/oembed.json"), 0, "robots 禁止时不得请求");
    let channel = &envelope["data"]["feed_channels"][0];
    assert_eq!(channel["status"], "blocked");
    assert_eq!(envelope["code"], "DEGRADED");
    assert_eq!(envelope["meta"]["degraded_from"], "channel-fetch-failed");
    assert_eq!(envelope["meta"]["actionable"]["reason"], "feed-unavailable");
}

/// sitemap 默认关（零请求）；opt-in 且 robots 禁止时不请求并显式降级。
#[tokio::test]
async fn sitemap_is_off_by_default_and_blocked_when_disallowed() {
    let server = http_fixture::Server::start_with(vec![
        ("/page", http_fixture::Response::html(PAGE)),
        ("/sitemap.xml", http_fixture::Response::xml(SITEMAP_URLSET)),
    ])
    .await;
    let mcp = WebctlMcp::new(in_process_context("sitemap-default"));
    let off = invoke(&mcp, "embedded_data", json!({"url": server.url("/page")})).await;
    assert_eq!(off["ok"], true, "{off}");
    assert!(
        off["data"]["sitemap"].is_null(),
        "默认不返回 sitemap：{off}"
    );
    assert_eq!(server.count("/sitemap.xml"), 0, "默认不得请求 sitemap");
    assert_eq!(server.count("/robots.txt"), 0, "默认不得请求 robots");
    server.route(
        "/robots.txt",
        http_fixture::Response::robots("User-agent: *\nDisallow: /sitemap.xml\n"),
    );
    let blocked = invoke(
        &mcp,
        "embedded_data",
        json!({"url": server.url("/page"), "fetch_sitemap": true}),
    )
    .await;
    assert_eq!(server.count("/sitemap.xml"), 0, "robots 禁止时不得请求");
    assert_eq!(blocked["data"]["sitemap"]["status"], "blocked");
    assert_eq!(blocked["code"], "DEGRADED");
    assert_eq!(blocked["meta"]["degraded_from"], "sitemap-fetch-failed");
    assert_eq!(
        blocked["meta"]["actionable"]["reason"],
        "sitemap-unavailable"
    );
}

/// 显式 opt-in：robots 声明的 Sitemap: 优先，不去猜 /sitemap.xml。
#[tokio::test]
async fn sitemap_opt_in_prefers_robots_declaration() {
    let server = http_fixture::Server::start().await;
    server.route("/page", http_fixture::Response::html(PAGE));
    server.route(
        "/robots.txt",
        http_fixture::Response::robots(&format!(
            "User-agent: *\nSitemap: {}\n",
            server.url("/site.xml")
        )),
    );
    server.route("/site.xml", http_fixture::Response::xml(SITEMAP_URLSET));
    server.route("/sitemap.xml", http_fixture::Response::xml(SITEMAP_URLSET));
    let mcp = WebctlMcp::new(in_process_context("sitemap-robots"));
    let envelope = invoke(
        &mcp,
        "embedded_data",
        json!({"url": server.url("/page"), "fetch_sitemap": true}),
    )
    .await;
    let sitemap = &envelope["data"]["sitemap"];
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(sitemap["source"], "robots");
    assert_eq!(sitemap["status"], "hit");
    assert_eq!(sitemap["urls"][0], "https://example.test/a");
    assert_eq!(sitemap["urls"][1], "https://example.test/b");
    assert_eq!(server.count("/site.xml"), 1);
    assert_eq!(server.count("/sitemap.xml"), 0, "robots 声明优先");
}

/// sitemap index：递归取回子 sitemap（层数/条目有上限），只列 URL 不抓页面。
#[tokio::test]
async fn sitemap_index_is_recursed() {
    let server = http_fixture::Server::start().await;
    server.route("/page", http_fixture::Response::html(PAGE));
    server.route(
        "/robots.txt",
        http_fixture::Response::robots(&format!(
            "User-agent: *\nSitemap: {}\n",
            server.url("/index.xml")
        )),
    );
    server.route(
        "/index.xml",
        http_fixture::Response::xml(format!(
            "<sitemapindex><sitemap><loc>{}</loc></sitemap><sitemap><loc>{}</loc></sitemap></sitemapindex>",
            server.url("/a.xml"),
            server.url("/b.xml")
        )),
    );
    server.route(
        "/a.xml",
        http_fixture::Response::xml(
            "<urlset><url><loc>https://example.test/a1</loc></url><url><loc>https://example.test/a2</loc></url></urlset>",
        ),
    );
    server.route(
        "/b.xml",
        http_fixture::Response::xml(
            "<urlset><url><loc>https://example.test/b1</loc></url></urlset>",
        ),
    );
    let mcp = WebctlMcp::new(in_process_context("sitemap-index"));
    let envelope = invoke(
        &mcp,
        "embedded_data",
        json!({"url": server.url("/page"), "fetch_sitemap": true}),
    )
    .await;
    let sitemap = &envelope["data"]["sitemap"];
    assert_eq!(sitemap["status"], "hit", "{envelope}");
    assert_eq!(sitemap["documents"].as_array().expect("docs").len(), 3);
    assert_eq!(sitemap["urls"].as_array().expect("urls").len(), 3);
    assert_eq!(server.count("/a.xml"), 1);
    assert_eq!(server.count("/b.xml"), 1);
    assert_eq!(server.count("/a1"), 0, "只列 URL，不抓取页面");
}
