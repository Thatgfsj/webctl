//! web_search 紧凑性（0.2.3 M3，议题 #223）：同域折叠 + snippet 截断。
//!
//! 全部离线：请求只打 127.0.0.1 的合成 SERP fixture（`with_search_base`），零外网；
//! fixture 是手写合成的最小结构，不含真实站点内容。

mod common;

use common::http_fixture;
use common::in_process_context;
use serde_json::{Value, json};
use webctl_mcp::WebctlMcp;

/// 合成 Bing SERP：每条给出 (URL, 标题, snippet)。
fn bing_serp(items: &[(&str, &str, &str)]) -> String {
    let body: String = items
        .iter()
        .map(|(url, title, snippet)| {
            format!(
                r#"<li class="b_algo"><h2><a href="{url}">{title}</a></h2><div class="b_caption"><p>{snippet}</p></div><cite>example.com</cite></li>"#
            )
        })
        .collect();
    format!(r#"<html><body><ol id="b_results">{body}</ol></body></html>"#)
}

async fn server_with(html: String) -> http_fixture::Server {
    http_fixture::Server::start_with(vec![("/search", http_fixture::Response::html(html))]).await
}

async fn call(mcp: &WebctlMcp, args: Value) -> Value {
    mcp.invoke("web_search", args)
        .await
        .unwrap_or_else(|| panic!("web_search 未注册"))
}

fn hits(envelope: &Value) -> &Vec<Value> {
    envelope["data"]["results"]
        .as_array()
        .unwrap_or_else(|| panic!("results 必须是数组：{envelope}"))
}

fn snippet_chars(hit: &Value) -> usize {
    hit["snippet"].as_str().expect("snippet").chars().count()
}

/// a) 默认折叠：同域 2 条 → 1 条主结果 + same_domain_count=2。
#[tokio::test]
async fn same_domain_results_collapse_by_default() {
    let server = server_with(bing_serp(&[
        ("https://example.com/a", "A", "snippet a"),
        ("https://example.com/b", "B", "snippet b"),
        ("https://other.test/c", "C", "snippet c"),
    ]))
    .await;
    let mcp = WebctlMcp::new(
        in_process_context("compact-collapse").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(&mcp, json!({"query": "rust"})).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    let results = hits(&envelope);
    assert_eq!(results.len(), 2, "{envelope}");
    assert_eq!(results[0]["url"], "https://example.com/a");
    assert_eq!(results[0]["rank"], 1);
    assert_eq!(results[0]["same_domain_count"], 2);
    assert!(
        results[1].get("same_domain_count").is_none(),
        "未折叠的域不应出现 same_domain_count：{}",
        results[1]
    );
    assert_eq!(results[1]["url"], "https://other.test/c");
}

/// b) expand_domains=true 还原完整列表。
#[tokio::test]
async fn expand_domains_restores_same_domain_results() {
    let server = server_with(bing_serp(&[
        ("https://example.com/a", "A", "snippet a"),
        ("https://example.com/b", "B", "snippet b"),
        ("https://other.test/c", "C", "snippet c"),
    ]))
    .await;
    let mcp = WebctlMcp::new(
        in_process_context("compact-expand").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(&mcp, json!({"query": "rust", "expand_domains": true})).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    let results = hits(&envelope);
    assert_eq!(results.len(), 3, "{envelope}");
    assert!(
        results
            .iter()
            .all(|hit| hit.get("same_domain_count").is_none()),
        "展开时不应出现 same_domain_count：{envelope}"
    );
}

/// c) 默认 300 字符截断：中文字符串按 Unicode 计数，不按字节。
#[tokio::test]
async fn long_snippet_is_truncated_to_300_unicode_chars_by_default() {
    let long = "中".repeat(301);
    let server = server_with(bing_serp(&[
        ("https://example.com/long", "长摘要", &long),
        ("https://other.test/c", "短摘要", "短"),
    ]))
    .await;
    let mcp = WebctlMcp::new(
        in_process_context("compact-300").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(&mcp, json!({"query": "rust"})).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    let results = hits(&envelope);
    assert_eq!(results[0]["snippet_truncated"], true, "{envelope}");
    assert_eq!(snippet_chars(&results[0]), 300, "{envelope}");
    assert!(
        results[0]["snippet"]
            .as_str()
            .expect("snippet")
            .chars()
            .all(|ch| ch == '中'),
        "截断后必须是合法 UTF-8，不得出现半个字符"
    );
    assert!(
        results[1].get("snippet_truncated").is_none(),
        "未超长的 snippet 不应带 snippet_truncated：{}",
        results[1]
    );
}

/// d) snippet_max_chars=50 生效；传 0 表示不截断。
#[tokio::test]
async fn snippet_max_chars_is_configurable() {
    let long = "中".repeat(301);
    let server = server_with(bing_serp(&[("https://example.com/long", "长摘要", &long)])).await;
    let mcp = WebctlMcp::new(
        in_process_context("compact-50").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(&mcp, json!({"query": "rust", "snippet_max_chars": 50})).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    let results = hits(&envelope);
    assert_eq!(snippet_chars(&results[0]), 50, "{envelope}");
    assert_eq!(results[0]["snippet_truncated"], true, "{envelope}");

    let untouched = call(&mcp, json!({"query": "rust", "snippet_max_chars": 0})).await;
    assert_eq!(untouched["ok"], true, "{untouched}");
    let results = hits(&untouched);
    assert_eq!(snippet_chars(&results[0]), 301, "{untouched}");
    assert!(
        results[0].get("snippet_truncated").is_none(),
        "limit=0 不截断：{untouched}"
    );
}

/// e) 先折叠再截到 max_results：不同域的结果不会被同域条目挤掉。
///
/// 两个引擎各返回 2 条：第 1 名都在 a.test，第 2 名都在 b.test。RRF 后前两名
/// 都是 a.test 的不同 URL；若先截到 max_results 再折叠就只剩 1 条，正确顺序应保留两个域。
#[tokio::test]
async fn max_results_applies_after_domain_collapse() {
    const MULTI: &str = r#"<html><body>
<ol id="b_results">
  <li class="b_algo"><h2><a href="https://a.test/1">A1</a></h2>
    <div class="b_caption"><p>Bing A</p></div><cite>a.test</cite></li>
  <li class="b_algo"><h2><a href="https://b.test/1">B1</a></h2>
    <div class="b_caption"><p>Bing B</p></div><cite>b.test</cite></li>
</ol>
<ul class="result">
  <li class="res-list"><h3 class="res-title">
    <a data-mdurl="https://a.test/2" href="https://www.so.com/link?m=x">A2</a></h3>
    <p class="res-desc">360 A</p></li>
  <li class="res-list"><h3 class="res-title">
    <a data-mdurl="https://b.test/2" href="https://www.so.com/link?m=y">B2</a></h3>
    <p class="res-desc">360 B</p></li>
</ul>
</body></html>"#;
    let server = server_with(MULTI.to_owned()).await;
    let mcp = WebctlMcp::new(
        in_process_context("compact-max").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(
        &mcp,
        json!({"query": "rust", "engines": ["bing", "360"], "max_results": 2}),
    )
    .await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    let results = hits(&envelope);
    assert_eq!(results.len(), 2, "{envelope}");
    assert_eq!(results[0]["url"], "https://a.test/1");
    assert_eq!(results[0]["same_domain_count"], 2);
    assert_eq!(results[1]["url"], "https://b.test/1");
    assert_eq!(results[1]["same_domain_count"], 2);
    assert_eq!(results[0]["rank"], 1);
    assert_eq!(results[1]["rank"], 2);
}

/// f) 既有 I2 行为不回归：applied_operators / filtered_ads / is_ad 仍在。
#[tokio::test]
async fn i2_fields_still_reported_alongside_compactness() {
    // 高置信 bing.com/ck/a 广告（剔除并计数）+ 自然结果 + 低置信「广告」标签（保留打标）。
    const I2_SERP: &str = r#"<html><body><ol id="b_results">
<li class="b_algo"><h2><a href="https://www.bing.com/ck/a?!&amp;&amp;u=a1aHR0cHM6Ly9hZHMuZXhhbXBsZS5vcmcvbGFuZGluZw">特惠云主机</a></h2><div class="b_caption"><p>限时优惠。</p></div><cite>ads.example.org</cite></li>
<li class="b_algo"><h2><a href="https://example.com/docs">Rust 官方文档</a></h2><div class="b_caption"><p>语言与标准库文档。</p></div><cite>example.com</cite></li>
<li class="b_algo"><h2><a href="https://example.net/course">广告 · Rust 培训课程</a></h2><div class="b_caption"><p>商业培训。</p></div><cite>example.net</cite></li>
</ol></body></html>"#;
    let server = server_with(I2_SERP.to_owned()).await;
    let mcp = WebctlMcp::new(
        in_process_context("compact-i2").with_search_base(Some(server.url("/search"))),
    );
    let envelope = call(&mcp, json!({"query": "rust", "exclude": ["python"]})).await;
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["code"], "OK", "{envelope}");
    assert_eq!(envelope["data"]["filtered_ads"], 1, "{envelope}");
    assert_eq!(
        envelope["data"]["applied_operators"],
        json!(["-python"]),
        "{envelope}"
    );
    let results = hits(&envelope);
    assert_eq!(results.len(), 2, "{envelope}");
    assert!(
        results.iter().all(|hit| !hit["url"]
            .as_str()
            .unwrap_or_default()
            .contains("ads.example.org")),
        "高置信广告不得出现在结果里：{envelope}"
    );
    assert_eq!(
        results.iter().filter(|hit| hit["is_ad"] == true).count(),
        1,
        "低置信广告应保留并打标：{envelope}"
    );
}
