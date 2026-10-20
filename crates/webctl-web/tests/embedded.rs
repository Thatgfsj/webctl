//! EMBED-01…EMBED-13：内嵌结构化数据抽取的离线用例（议题 #114 / EPIC #113）。
//!
//! 全部用例只读内存里的 HTML 字符串：不联网、不写盘、不启动子进程、不执行 JS。
//! 语义要点：**零结果 ≠ 解析失败**；找到容器但一条都解析不出来才是 PARSE；
//! 截断要显式标注且不假装解析成功；IIFE 不是 JSON，必须如实报错而不是硬猜。

use webctl_core::ErrorCode;
use webctl_web::parse::{EmbeddedOptions, extract_embedded};

fn entries(html: &str) -> Vec<webctl_web::parse::EmbeddedEntry> {
    let envelope = extract_embedded(html, &EmbeddedOptions::default());
    assert!(envelope.ok, "envelope not ok: {:?}", envelope.error);
    envelope.data.expect("data").entries
}

/// EMBED-01：单块 JSON-LD 被解析成可寻址的 JSON。
#[test]
fn embed_01_single_ld_json() {
    let html = r#"<html><head><script type="application/ld+json">
      {"@context":"https://schema.org","@type":"Article","headline":"标题"}
    </script></head></html>"#;
    let list = entries(html);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].source, "ld+json");
    assert_eq!(list[0].value.as_ref().unwrap()["headline"], "标题");
}

/// EMBED-02：同页多块 JSON-LD（含数组形式）全部保序返回。
#[test]
fn embed_02_multiple_ld_json_blocks_keep_order() {
    let html = r#"<script type="application/ld+json">{"a":1}</script>
    <script type="application/ld+json">[{"b":2},{"b":3}]</script>"#;
    let list = entries(html);
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].value.as_ref().unwrap()["a"], 1);
    assert_eq!(list[1].value.as_ref().unwrap().as_array().unwrap().len(), 2);
}

/// EMBED-03：`__NEXT_DATA__` 这类 `application/json` + id 的容器。
#[test]
fn embed_03_next_data_container() {
    let html = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"x":42}}}</script>"#;
    let list = entries(html);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].source, "script-json");
    assert_eq!(list[0].name.as_deref(), Some("__NEXT_DATA__"));
    assert_eq!(
        list[0].value.as_ref().unwrap()["props"]["pageProps"]["x"],
        42
    );
}

/// EMBED-04：内联状态赋值里的对象字面量。
#[test]
fn embed_04_inline_state_object() {
    let html = r#"<script>window.__INITIAL_STATE__={"user":{"id":7},"items":[1,2]};</script>"#;
    let list = entries(html);
    assert_eq!(list.len(), 1, "同一处赋值不得因变量表重叠而重复: {list:?}");
    assert_eq!(list[0].source, "inline-state");
    assert_eq!(list[0].name.as_deref(), Some("window.__INITIAL_STATE__"));
    assert_eq!(list[0].value.as_ref().unwrap()["user"]["id"], 7);
}

/// EMBED-05：`window.__NUXT__` 常见形态是 **IIFE**，不是 JSON —— 必须如实报错，不许硬猜。
#[test]
fn embed_05_iife_is_reported_not_guessed() {
    let html = r#"<script>window.__NUXT__=(function(a,b){return {data:a}})(1,2);</script>"#;
    let list = entries(html);
    // 括号字面量扫描不接受 `(` 开头，因此这条根本不会被当成载荷 —— 零结果不是失败。
    assert!(list.is_empty(), "IIFE 不该被当成 JSON 载荷: {list:?}");
}

/// EMBED-06：容器在但 JSON 坏了 —— 全失败时信封必须是 PARSE。
#[test]
fn embed_06_broken_json_yields_parse_error() {
    let html = r#"<script type="application/ld+json">{"a": }</script>"#;
    let envelope = extract_embedded(html, &EmbeddedOptions::default());
    assert!(!envelope.ok, "坏 JSON 不该 ok");
    assert_eq!(envelope.code, ErrorCode::Parse);
    assert!(
        envelope.error.contains("ld+json"),
        "错误要指出来源: {}",
        envelope.error
    );
}

/// EMBED-07：Inertia 的 `data-page`（HTML 实体转义后的 JSON）。
#[test]
fn embed_07_data_page_with_entities() {
    let html = r#"<div id="app" data-page="{&quot;component&quot;:&quot;Users/Index&quot;,&quot;props&quot;:{&quot;n&quot;:3}}"></div>"#;
    let list = entries(html);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].source, "data-page");
    assert_eq!(list[0].value.as_ref().unwrap()["component"], "Users/Index");
}

/// EMBED-08：页面确实没有内嵌数据 —— 合法零结果，而不是 PARSE。
#[test]
fn embed_08_no_embedded_data_is_valid_empty_result() {
    let html = r#"<html><body><p>纯静态内容</p><script src="/app.js"></script></body></html>"#;
    let envelope = extract_embedded(html, &EmbeddedOptions::default());
    assert!(envelope.ok, "零结果应当 ok=true");
    assert_eq!(envelope.code, ErrorCode::Ok);
    let report = envelope.data.unwrap();
    assert!(report.entries.is_empty());
    assert_eq!(report.scanned_scripts, 1, "要如实报告看过几个 script");
    assert!(
        report.conclusion.contains("valid empty result"),
        "{}",
        report.conclusion
    );
}

/// EMBED-09：超过字节上限 —— 显式 truncated，且**不**解析。
#[test]
fn embed_09_oversized_payload_is_marked_truncated() {
    let big = "x".repeat(4096);
    let html = format!(r#"<script type="application/json">{{"blob":"{big}"}}</script>"#);
    let options = EmbeddedOptions {
        max_entry_bytes: 256,
        ..EmbeddedOptions::default()
    };
    let envelope = extract_embedded(&html, &options);
    let list = envelope.data.unwrap().entries;
    assert_eq!(list.len(), 1);
    assert!(list[0].truncated);
    assert!(list[0].value.is_none(), "截断时不得假装解析成功");
    assert!(list[0].parse_error.is_none(), "截断不是解析失败");
    assert!(list[0].bytes > 256, "bytes 要记原始长度");
}

/// EMBED-10：UTF-8 多字节内容不得让扫描器 panic（切片边界安全）。
#[test]
fn embed_10_multibyte_content_is_safe() {
    let html = r#"<html><body><p>中文段落，用于测试字符边界安全性。</p>
      <script id="__NEXT_DATA__" type="application/json">{"标题":"中文","值":1}</script>
      <div data-page="{&quot;组件&quot;:&quot;中文/页面&quot;}"></div></body></html>"#;
    let list = entries(html);
    assert!(list.len() >= 2);
    assert!(list.iter().any(|e| e.value.is_some()));
}

/// EMBED-11：`type` 带参数（`;charset=utf-8`）仍要认出 JSON-LD。
#[test]
fn embed_11_ld_json_with_charset() {
    let html = r#"<script type="application/ld+json;charset=utf-8">{"@type":"Product"}</script>"#;
    let list = entries(html);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].source, "ld+json");
}

/// EMBED-12：单条上限与条数上限都起作用（防畸形页面撑爆结果）。
#[test]
fn embed_12_entry_count_is_capped() {
    let mut html = String::new();
    for i in 0..64 {
        html.push_str(&format!(
            r#"<script type="application/ld+json">{{"i":{i}}}</script>"#
        ));
    }
    let options = EmbeddedOptions {
        max_entries: 5,
        ..EmbeddedOptions::default()
    };
    let list = extract_embedded(&html, &options).data.unwrap().entries;
    assert_eq!(list.len(), 5);
}

/// EMBED-13：`window.X` 与裸 `X` 两种写法**各自出现**时，应是两条而不是漏掉一条。
#[test]
fn embed_13_bare_and_window_prefixed_are_distinct_when_separate() {
    let html = r#"<script>window.__INITIAL_STATE__={"a":1};</script>
      <script>__INITIAL_STATE__={"b":2};</script>"#;
    let list = entries(html);
    assert_eq!(list.len(), 2, "两处不同的赋值都要拿到: {list:?}");
    assert_eq!(list[0].value.as_ref().unwrap()["a"], 1);
    assert_eq!(list[1].value.as_ref().unwrap()["b"], 2);
}

/// #117 接线第 1 半：元数据必须带 `source`，且**不并进正文/条目**。
#[test]
fn report_exposes_metadata_with_explicit_source() {
    let html = r#"<html><head>
      <title>标题</title>
      <meta property="og:title" content="OG">
      <meta name="twitter:card" content="summary">
    </head><body>正文内容</body></html>"#;
    let report = extract_embedded(html, &EmbeddedOptions::default());
    assert!(report.ok, "{report:?}");
    let data = report.data.expect("data");
    assert_eq!(data.metadata.title.as_deref(), Some("标题"));
    let og = data
        .metadata
        .entries
        .iter()
        .find(|e| e.key == "og:title")
        .expect("og:title 应在元数据里");
    assert_eq!(og.source, "og-meta", "元数据必须带来源标签");
    assert!(
        data.entries.iter().all(|e| e.source != "og-meta"),
        "元数据不得混进结构化数据条目"
    );
    // #153 复审指出：原描述声称"正文不进元数据"，但当时没有直接断言（描述夸大）。
    // 现在补上：正文标记不得出现在 metadata 或 channels 的任何序列化结果里。
    let serialized = format!(
        "{}{}",
        serde_json::to_string(&data.metadata).expect("metadata 可序列化"),
        serde_json::to_string(&data.channels).expect("channels 可序列化"),
    );
    assert!(
        !serialized.contains("正文内容"),
        "正文不得出现在元数据/通道里：{serialized}"
    );
}

/// #117：通道只报告**声明**（不拉取网络）；没有声明就是空。
#[test]
fn report_exposes_declared_channels_only() {
    let html = r#"<html><head>
      <link rel="alternate" type="application/rss+xml" href="/feed.xml">
      <link rel="alternate" type="application/json+oembed" href="/oembed">
    </head><body>x</body></html>"#;
    let report = extract_embedded(html, &EmbeddedOptions::default());
    let data = report.data.expect("data");
    assert_eq!(data.channels.len(), 2, "应报告 2 条声明通道");
    assert_eq!(data.channels[0].href, "/feed.xml");
    let plain = extract_embedded(
        "<html><body>无声明</body></html>",
        &EmbeddedOptions::default(),
    );
    assert!(
        plain.data.expect("data").channels.is_empty(),
        "没有声明就是空，不猜"
    );
}
