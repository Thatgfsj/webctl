//! HTTP-01…HTTP-18、信封与 cookie 往返的离线集成测试。
//!
//! 全部用例只访问 `127.0.0.1` 上的本地 fixture 服务，不访问外网、不启动子进程。
//! 用例编号对齐 docs/TESTING.md §2.3。

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use support::{Route, Server};
use webctl_core::ErrorCode;
use webctl_web::transport::{FetchOptions, fetch};
use webctl_web::{CharsetSource, CookieJar, StoredCookie};

const GBK_BYTES: &[u8] = &[
    0xd6, 0xd0, 0xce, 0xc4, 0xb1, 0xe0, 0xc2, 0xeb, 0xb2, 0xe2, 0xca, 0xd4, 0xa3, 0xba, 0xc4, 0xe3,
    0xba, 0xc3, 0xa3, 0xac, 0xca, 0xc0, 0xbd, 0xe7,
];
const SJIS_BYTES: &[u8] = &[
    0x93, 0xfa, 0x96, 0x7b, 0x8c, 0xea, 0x82, 0xcc, 0x83, 0x65, 0x83, 0x58, 0x83, 0x67, 0x81, 0x46,
    0x82, 0xb1, 0x82, 0xf1, 0x82, 0xc9, 0x82, 0xbf, 0x82, 0xcd,
];
const GZIP_BYTES: &[u8] = &[
    0x1f, 0x8b, 0x08, 0x00, 0xde, 0x49, 0xd2, 0x6a, 0x02, 0xff, 0x7b, 0xda, 0xd7, 0xfd, 0x7c, 0xcf,
    0xca, 0xa7, 0x13, 0xfa, 0x9e, 0xcf, 0x6a, 0x79, 0xb6, 0x76, 0xf1, 0xb3, 0x69, 0xed, 0x0a, 0xe9,
    0x55, 0x99, 0x05, 0x0a, 0xf9, 0xd9, 0x00, 0xe0, 0x48, 0x7c, 0xc5, 0x1a, 0x00, 0x00, 0x00,
];
const BROTLI_BYTES: &[u8] = &[
    0x8b, 0x0d, 0x80, 0xe5, 0x8e, 0x8b, 0xe7, 0xbc, 0xa9, 0xe5, 0x90, 0x8e, 0xe7, 0x9a, 0x84, 0xe6,
    0xad, 0xa3, 0xe6, 0x96, 0x87, 0x20, 0x62, 0x72, 0x6f, 0x74, 0x6c, 0x69, 0x20, 0x6f, 0x6b, 0x03,
];

fn options() -> FetchOptions {
    FetchOptions::default()
}

/// HTTP-01：UTF-8 正文与 fixture 完全一致。
#[tokio::test]
async fn http_01_utf8_body_matches_fixture() {
    let text = "你好，世界 — UTF-8";
    let server = Server::start(vec![("/utf8", Route::html(text))]).await;
    let env = fetch(&server.url("/utf8"), &options()).await;
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.validate(), Ok(()));
    let data = env.data.as_ref().expect("data");
    assert_eq!(data.status, 200);
    assert_eq!(data.content.kind.as_str(), "html");
    assert_eq!(data.content.text.as_deref(), Some(text));
    assert_eq!(data.content.charset.as_deref(), Some("UTF-8"));
    assert_eq!(
        data.content.charset_source,
        Some(CharsetSource::ContentType)
    );
}

/// HTTP-02：GBK 解码。
#[tokio::test]
async fn http_02_gbk_decodes() {
    let server = Server::start(vec![(
        "/gbk",
        Route::new(200, GBK_BYTES.to_vec()).header("content-type", "text/html; charset=gbk"),
    )])
    .await;
    let env = fetch(&server.url("/gbk"), &options()).await;
    let data = env.data.as_ref().expect("data");
    assert_eq!(
        data.content.text.as_deref(),
        Some("中文编码测试：你好，世界")
    );
    assert_eq!(data.content.charset.as_deref(), Some("GBK"));
}

/// HTTP-03：Shift_JIS 解码。
#[tokio::test]
async fn http_03_shift_jis_decodes() {
    let server = Server::start(vec![(
        "/sjis",
        Route::new(200, SJIS_BYTES.to_vec()).header("content-type", "text/html; charset=shift_jis"),
    )])
    .await;
    let env = fetch(&server.url("/sjis"), &options()).await;
    let data = env.data.as_ref().expect("data");
    assert_eq!(
        data.content.text.as_deref(),
        Some("日本語のテスト：こんにちは")
    );
    assert_eq!(data.content.charset.as_deref(), Some("Shift_JIS"));
}

/// HTTP-04：无 charset 声明时有确定回退，且编码元数据可用。
#[tokio::test]
async fn http_04_missing_charset_falls_back_deterministically() {
    let mut meta_body = b"<html><head><meta charset=gbk></head><body>".to_vec();
    meta_body.extend_from_slice(GBK_BYTES);
    let server = Server::start(vec![
        (
            "/no-charset",
            Route::ok("fallback ok").header("content-type", "text/html"),
        ),
        ("/meta", Route::new(200, meta_body)),
    ])
    .await;
    let env = fetch(&server.url("/no-charset"), &options()).await;
    let data = env.data.as_ref().expect("data");
    assert_eq!(data.content.text.as_deref(), Some("fallback ok"));
    assert_eq!(data.content.charset_source, Some(CharsetSource::Utf8Valid));
    assert_eq!(data.content.charset.as_deref(), Some("UTF-8"));

    let meta = fetch(&server.url("/meta"), &options()).await;
    let data = meta.data.as_ref().expect("data");
    assert_eq!(data.content.charset_source, Some(CharsetSource::HtmlMeta));
    assert_eq!(data.content.charset.as_deref(), Some("GBK"));
}

/// HTTP-05：gzip 解压后与原文一致。
#[tokio::test]
async fn http_05_gzip_is_decompressed() {
    let server = Server::start(vec![(
        "/gzip",
        Route::html(GZIP_BYTES.to_vec()).header("content-encoding", "gzip"),
    )])
    .await;
    let env = fetch(&server.url("/gzip"), &options()).await;
    assert!(env.ok, "{}", env.error);
    let data = env.data.as_ref().expect("data");
    assert_eq!(data.content.text.as_deref(), Some("压缩后的正文 gzip ok"));
}

/// HTTP-06：brotli 解压后与原文一致。
#[tokio::test]
async fn http_06_brotli_is_decompressed() {
    let server = Server::start(vec![(
        "/brotli",
        Route::html(BROTLI_BYTES.to_vec()).header("content-encoding", "br"),
    )])
    .await;
    let env = fetch(&server.url("/brotli"), &options()).await;
    assert!(env.ok, "{}", env.error);
    let data = env.data.as_ref().expect("data");
    assert_eq!(data.content.text.as_deref(), Some("压缩后的正文 brotli ok"));
}

/// HTTP-07：3 跳重定向，逐跳 URL / 状态码在信封中可见。
#[tokio::test]
async fn http_07_three_hop_redirect_chain_is_visible() {
    let server = Server::start(vec![
        ("/r1", Route::redirect(301, "/r2")),
        ("/r2", Route::redirect(302, "/r3")),
        ("/r3", Route::redirect(302, "/final")),
        ("/final", Route::html("done")),
    ])
    .await;
    let env = fetch(&server.url("/r1"), &options()).await;
    assert!(env.ok, "{}", env.error);
    let data = env.data.as_ref().expect("data");
    assert_eq!(data.redirects.len(), 3, "链长必须是 3");
    assert_eq!(
        data.redirects
            .iter()
            .map(|hop| hop.status)
            .collect::<Vec<_>>(),
        vec![301, 302, 302]
    );
    assert_eq!(data.redirects[0].url, server.url("/r1"));
    assert_eq!(data.redirects[0].location, server.url("/r2"));
    assert_eq!(data.redirects[2].location, server.url("/final"));
    assert_eq!(data.final_url, server.url("/final"));
    assert_eq!(data.status, 200);
    assert_eq!(data.content.text.as_deref(), Some("done"));
}

/// HTTP-08：相对 Location 解析为绝对 URL。
#[tokio::test]
async fn http_08_relative_location_is_resolved() {
    let server = Server::start(vec![
        ("/rel", Route::redirect(302, "sub/target")),
        ("/sub/target", Route::html("relative ok")),
        ("/a/b/rel", Route::redirect(302, "../target")),
        ("/a/target", Route::html("dotdot ok")),
    ])
    .await;

    let env = fetch(&server.url("/rel"), &options()).await;
    let data = env.data.as_ref().expect("data");
    assert_eq!(data.final_url, server.url("/sub/target"));
    assert_eq!(data.redirects[0].location, server.url("/sub/target"));

    let env = fetch(&server.url("/a/b/rel"), &options()).await;
    let data = env.data.as_ref().expect("data");
    assert_eq!(data.final_url, server.url("/a/target"));
}

/// HTTP-09：重定向环必须终止并给出明确错误；max-hops 对长链同样生效。
#[tokio::test]
async fn http_09_redirect_loop_terminates_with_error() {
    let server = Server::start(vec![
        ("/loop-a", Route::redirect(302, "/loop-b")),
        ("/loop-b", Route::redirect(302, "/loop-a")),
    ])
    .await;
    let env = fetch(&server.url("/loop-a"), &options()).await;
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Network);
    assert!(
        env.error.contains("redirect loop") || env.error.contains("redirect limit"),
        "错误必须说明重定向问题：{}",
        env.error
    );
    assert!(
        server.total() <= options().max_redirects + 1,
        "请求数必须有界"
    );
    assert_eq!(env.validate(), Ok(()));

    let routes: Vec<(String, Route)> = (0..10)
        .map(|i| {
            (
                format!("/c{}", i),
                Route::redirect(302, format!("/c{}", i + 1)),
            )
        })
        .collect();
    let chain = Server::start(routes).await;
    let mut bounded = options();
    bounded.max_redirects = 2;
    let env = fetch(&chain.url("/c0"), &bounded).await;
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Network);
    assert!(env.error.contains("redirect limit"), "{}", env.error);
    assert!(chain.total() <= 4, "max-hops 后不得继续请求");
}

/// HTTP-10：cookie jar 在同一会话内跨请求生效。
#[tokio::test]
async fn http_10_cookie_jar_replays_cookie() {
    let server = Server::start(vec![
        (
            "/set",
            Route::ok("set").header("set-cookie", "sid=abc123; Path=/"),
        ),
        ("/echo", Route::ok("echoed")),
    ])
    .await;
    let jar = Arc::new(CookieJar::new());
    let opts = options().with_cookie_jar(Arc::clone(&jar));
    assert!(fetch(&server.url("/set"), &opts).await.ok);
    assert!(fetch(&server.url("/echo"), &opts).await.ok);
    assert_eq!(server.cookie("/echo").as_deref(), Some("sid=abc123"));
}

/// HTTP-11：Domain / Path / Secure 约束在真实请求路径上生效。
#[tokio::test]
async fn http_11_cookie_constraints_are_enforced() {
    let server = Server::start(vec![
        (
            "/set-attrs",
            Route::ok("set")
                .header("set-cookie", "pathcake=1; Path=/app")
                .header("set-cookie", "evil=1; Domain=example.com; Path=/")
                .header("set-cookie", "sec=1; Secure; Path=/"),
        ),
        ("/app/echo", Route::ok("app")),
        ("/other/echo", Route::ok("other")),
    ])
    .await;
    let jar = Arc::new(CookieJar::new());
    let opts = options().with_cookie_jar(Arc::clone(&jar));
    assert!(fetch(&server.url("/set-attrs"), &opts).await.ok);
    assert!(fetch(&server.url("/app/echo"), &opts).await.ok);
    assert!(fetch(&server.url("/other/echo"), &opts).await.ok);

    let app = server.cookie("/app/echo").unwrap_or_default();
    assert!(app.contains("pathcake=1"), "Path=/app 应下发：{app}");
    assert!(!app.contains("evil=1"), "Domain 越界不得下发：{app}");
    assert!(!app.contains("sec=1"), "Secure 在 http 下不得下发：{app}");
    assert!(server.cookie("/other/echo").is_none(), "Path 越界不得下发");
}

/// cookie 中间结构可序列化往返（HTTP-10 的持久化前提）。
#[tokio::test]
async fn cookie_snapshot_roundtrips_through_json() {
    let jar = CookieJar::new();
    jar.store(
        "k=v; Domain=.example.test; Path=/a; HttpOnly",
        &url::Url::parse("https://www.example.test/a/b").expect("url"),
    );
    let snapshot = jar.snapshot();
    let encoded = serde_json::to_string(&snapshot).expect("serialize snapshot");
    let decoded: Vec<StoredCookie> = serde_json::from_str(&encoded).expect("deserialize snapshot");
    let restored = CookieJar::from_snapshot(decoded);
    assert_eq!(restored.snapshot(), snapshot);
    assert_eq!(
        restored
            .cookie_header(&url::Url::parse("https://api.example.test/a/x").expect("url"))
            .as_deref(),
        Some("k=v")
    );
}

/// HTTP-12：超时 → TIMEOUT；超时后连接释放，后续请求仍可用（RES-02）。
#[tokio::test]
async fn http_12_timeout_maps_to_timeout_and_releases() {
    let server = Server::start(vec![
        ("/slow", Route::ok("slow").delay(Duration::from_secs(2))),
        ("/fast", Route::ok("fast")),
    ])
    .await;
    let mut opts = options();
    opts.timeout = Duration::from_millis(200);
    let started = Instant::now();
    let env = fetch(&server.url("/slow"), &opts).await;
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "必须按超时返回而不是等满延迟"
    );
    assert_eq!(env.validate(), Ok(()));

    let recovered = fetch(&server.url("/fast"), &opts).await;
    assert!(recovered.ok, "超时后连接应被释放：{}", recovered.error);
}

/// HTTP-13：连接失败 → NETWORK。
#[tokio::test]
async fn http_13_connection_failure_is_network() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let env = fetch(&format!("http://{addr}/"), &options()).await;
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Network);
    assert!(!env.error.is_empty());
}

/// HTTP-14：4xx/5xx 归类为 BLOCKED，且不与 PARSE 混用。
#[tokio::test]
async fn http_14_http_error_statuses_are_blocked() {
    let server = Server::start(vec![
        ("/missing", Route::new(404, "no such page")),
        ("/boom", Route::new(500, "server error")),
    ])
    .await;
    for (path, status) in [("/missing", 404), ("/boom", 500)] {
        let env = fetch(&server.url(path), &options()).await;
        assert!(!env.ok);
        assert_eq!(env.code, ErrorCode::Blocked, "{path}");
        assert_ne!(env.code, ErrorCode::Parse);
        assert!(env.data.is_none());
        assert!(env.error.contains(&status.to_string()), "{}", env.error);
    }
}

/// HTTP-15：内容类型 html/json/xml/pdf/binary 五分支各自命中。
#[tokio::test]
async fn http_15_content_type_routing_hits_five_branches() {
    let server = Server::start(vec![
        ("/html", Route::html("<html><body>hi</body></html>")),
        (
            "/json",
            Route::ok(r#"{"a":1}"#).header("content-type", "application/json"),
        ),
        (
            "/xml",
            Route::ok("<r/>").header("content-type", "application/xml"),
        ),
        (
            "/pdf",
            Route::ok(b"%PDF-1.7").header("content-type", "application/pdf"),
        ),
        (
            "/bin",
            Route::ok([0x00u8, 0x01, 0x02]).header("content-type", "application/octet-stream"),
        ),
    ])
    .await;
    let expected = [
        ("/html", "html", true),
        ("/json", "json", true),
        ("/xml", "xml", true),
        ("/pdf", "pdf", false),
        ("/bin", "binary", false),
    ];
    for (path, kind, is_text) in expected {
        let env = fetch(&server.url(path), &options()).await;
        let data = env
            .data
            .as_ref()
            .unwrap_or_else(|| panic!("{path}: {}", env.error));
        assert_eq!(data.content.kind.as_str(), kind, "{path}");
        assert_eq!(data.content.text.is_some(), is_text, "{path}");
    }
}

/// HTTP-16：非法 URL → BAD_URL，且服务端请求计数为 0（未发起连接）。
#[tokio::test]
async fn http_16_invalid_url_never_connects() {
    let server = Server::start(vec![("/anything", Route::ok("x"))]).await;
    for bad in [
        "",
        "not a url",
        "/relative/path",
        "ftp://example.test/file",
        "file:///etc/passwd",
    ] {
        let env = fetch(bad, &options()).await;
        assert!(!env.ok, "{bad:?}");
        assert_eq!(env.code, ErrorCode::BadUrl, "{bad:?}");
        assert!(!env.error.is_empty());
        assert_eq!(env.validate(), Ok(()));
    }
    assert_eq!(server.total(), 0, "非法 URL 不得发起任何请求");
}

/// HTTP-17：大响应被截断，带截断标记且 code=LIMIT。
#[tokio::test]
async fn http_17_large_response_is_truncated_with_limit() {
    let total = 256 * 1024;
    let server = Server::start(vec![("/large", Route::html(vec![b'A'; total]))]).await;
    let mut opts = options();
    opts.max_body_bytes = 4096;
    let env = fetch(&server.url("/large"), &opts).await;
    assert!(env.ok);
    assert_eq!(env.code, ErrorCode::Limit);
    assert_eq!(
        env.meta.degraded_from.as_deref(),
        Some("response-body-truncated")
    );
    assert_eq!(env.validate(), Ok(()));
    let data = env.data.as_ref().expect("data");
    assert!(data.content.truncated);
    assert_eq!(data.content.byte_len, 4096);
    assert_eq!(data.content.declared_len, Some(total as u64));
    assert_eq!(data.content.text.as_deref().map(str::len), Some(4096));
}

/// HTTP-18：二进制不当作文本解析。
#[tokio::test]
async fn http_18_binary_is_not_parsed_as_text() {
    let server = Server::start(vec![(
        "/bin",
        Route::ok([0x89u8, b'P', b'N', b'G', 0x00]).header("content-type", "image/png"),
    )])
    .await;
    let env = fetch(&server.url("/bin"), &options()).await;
    let data = env.data.as_ref().expect("data");
    assert_eq!(data.content.kind.as_str(), "binary");
    assert!(!data.content.kind.is_text());
    assert!(data.content.text.is_none());
    assert!(data.content.charset.is_none());
    assert_eq!(data.content.byte_len, 5);
    assert_eq!(env.validate(), Ok(()));
}

/// HTTP-01…18 的通用断言：信封形状与 trust 标记。
#[tokio::test]
async fn envelope_shape_and_trust_hold() {
    let server = Server::start(vec![
        ("/ok", Route::html("hi")),
        ("/missing", Route::new(404, "no")),
    ])
    .await;

    let ok = fetch(&server.url("/ok"), &options()).await;
    assert_eq!(ok.validate(), Ok(()));
    let value = serde_json::to_value(&ok).expect("json");
    assert_eq!(value["ok"], true);
    assert_eq!(value["code"], "OK");
    assert_eq!(value["error"], "");
    assert_eq!(value["meta"]["trust"], "untrusted_page_content");
    assert_eq!(value["meta"]["engine"], "builtin");
    assert_eq!(value["data"]["content"]["kind"], "html");
    assert!(value["meta"]["elapsed_ms"].is_u64());

    let failed = fetch(&server.url("/missing"), &options()).await;
    assert_eq!(failed.validate(), Ok(()));
    assert!(failed.data.is_none());
    assert!(!failed.error.is_empty());
    assert_eq!(
        serde_json::to_value(&failed).expect("json")["code"],
        "BLOCKED"
    );
}

/// A7：elapsed_ms 与实测墙钟同量级。
#[tokio::test]
async fn elapsed_ms_tracks_wall_clock() {
    let server = Server::start(vec![(
        "/slow",
        Route::ok("x").delay(Duration::from_millis(300)),
    )])
    .await;
    let wall = Instant::now();
    let env = fetch(&server.url("/slow"), &options()).await;
    let wall_ms = u64::try_from(wall.elapsed().as_millis()).unwrap_or(u64::MAX);
    let measured = env.meta.elapsed_ms;
    assert!(measured >= 250, "elapsed_ms={measured} 应覆盖 300ms 延迟");
    assert!(
        measured <= wall_ms + 100,
        "elapsed_ms={measured} wall={wall_ms}"
    );
}
