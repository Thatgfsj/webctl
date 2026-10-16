//! 议题 #22 §1：可注入 fixture HTTP 服务的自测（离线、零外网、零子进程）。
//!
//! 通过 #[path] 引入新支持模块，因此**不修改** tests/support/mod.rs。
//! 用例编号对齐 docs/TESTING.md §2.3（HTTP-*）与 §2.10（POLITE-*）的取回侧前置。

#[path = "support/http_fixture.rs"]
mod http_fixture;

use std::time::Duration;

use http_fixture::{Response, Server, read_fixture_hex, read_fixture_text};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use webctl_core::ErrorCode;
use webctl_web::transport::{FetchOptions, fetch};

fn options() -> FetchOptions {
    FetchOptions::default()
}

async fn raw_request(server: &Server, request: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(server.addr())
        .await
        .expect("connect fixture");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read response");
    response
}

/// fixture 只绑回环地址，且能按注册的路由返回字节。
#[tokio::test]
async fn http_fixture_binds_loopback_and_serves() {
    let server = Server::start_with(vec![("/hello", Response::html("hi"))]).await;
    assert!(server.addr().ip().is_loopback(), "fixture 只能绑 127.0.0.1");
    let env = fetch(&server.url("/hello"), &options()).await;
    assert!(env.ok, "{}", env.error);
    let data = env.data.as_ref().expect("data");
    assert_eq!(data.content.kind.as_str(), "html");
    assert_eq!(data.content.text.as_deref(), Some("hi"));
    assert_eq!(server.count("/hello"), 1);
    assert_eq!(server.total(), 1);
}

/// 可注入 GBK / Shift_JIS：样本从 tests/fixtures 读取并正确解码。
#[tokio::test]
async fn http_fixture_injects_gbk_and_shift_jis() {
    let gbk = read_fixture_hex("http/gbk.hex");
    let sjis = read_fixture_hex("http/shift_jis.hex");
    let server = Server::start_with(vec![
        (
            "/gbk",
            Response::new(200, gbk.clone()).header("content-type", "text/html; charset=gbk"),
        ),
        (
            "/sjis",
            Response::new(200, sjis.clone()).header("content-type", "text/html; charset=shift_jis"),
        ),
        (
            "/gbk-builder",
            Response::encoded_text("中文编码测试：你好，世界", "gbk"),
        ),
    ])
    .await;

    let env = fetch(&server.url("/gbk"), &options()).await;
    let data = env.data.as_ref().expect("gbk data");
    assert_eq!(data.content.charset.as_deref(), Some("GBK"));
    assert_eq!(
        data.content.text.as_deref(),
        Some("中文编码测试：你好，世界")
    );

    let env = fetch(&server.url("/sjis"), &options()).await;
    let data = env.data.as_ref().expect("sjis data");
    assert_eq!(data.content.charset.as_deref(), Some("Shift_JIS"));
    assert_eq!(
        data.content.text.as_deref(),
        Some("日本語のテスト：こんにちは")
    );

    let env = fetch(&server.url("/gbk-builder"), &options()).await;
    let data = env.data.as_ref().expect("builder data");
    assert_eq!(
        data.content.text.as_deref(),
        Some("中文编码测试：你好，世界")
    );
}

/// 可注入 gzip / brotli：样本已压缩，解码后与原文一致。
#[tokio::test]
async fn http_fixture_injects_gzip_and_brotli() {
    let gzip = read_fixture_hex("http/gzip.hex");
    let brotli = read_fixture_hex("http/brotli.hex");
    let server = Server::start_with(vec![
        ("/gzip", Response::gzip(gzip)),
        ("/br", Response::brotli(brotli)),
    ])
    .await;

    let env = fetch(&server.url("/gzip"), &options()).await;
    assert!(env.ok, "{}", env.error);
    assert_eq!(
        env.data
            .as_ref()
            .expect("gzip data")
            .content
            .text
            .as_deref(),
        Some("压缩后的正文 gzip ok")
    );

    let env = fetch(&server.url("/br"), &options()).await;
    assert!(env.ok, "{}", env.error);
    assert_eq!(
        env.data.as_ref().expect("br data").content.text.as_deref(),
        Some("压缩后的正文 brotli ok")
    );
}

/// 可注入多跳、相对 Location 与重定向环。
#[tokio::test]
async fn http_fixture_injects_redirects_and_loop() {
    let server = Server::start_with(vec![
        ("/r1", Response::redirect(301, "/r2")),
        ("/r2", Response::redirect(302, "sub/r3")),
        ("/sub/r3", Response::redirect(302, "/final")),
        ("/final", Response::html("done")),
        ("/loop-a", Response::redirect(302, "/loop-b")),
        ("/loop-b", Response::redirect(302, "/loop-a")),
    ])
    .await;

    let env = fetch(&server.url("/r1"), &options()).await;
    assert!(env.ok, "{}", env.error);
    let data = env.data.as_ref().expect("data");
    assert_eq!(data.redirects.len(), 3, "链长必须是 3");
    assert_eq!(data.redirects[1].location, server.url("/sub/r3"));
    assert_eq!(data.final_url, server.url("/final"));

    let env = fetch(&server.url("/loop-a"), &options()).await;
    assert!(!env.ok, "重定向环必须失败");
    assert_eq!(env.code, ErrorCode::Network);
    assert!(env.error.contains("loop"), "{}", env.error);
}

/// 可注入延迟：超过客户端超时就返回 TIMEOUT。
#[tokio::test]
async fn http_fixture_injects_delay_for_timeout() {
    let server = Server::start_with(vec![(
        "/slow",
        Response::text("late").delay(Duration::from_millis(500)),
    )])
    .await;
    let mut opts = options();
    opts.timeout = Duration::from_millis(50);
    let env = fetch(&server.url("/slow"), &opts).await;
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Timeout);
    assert_eq!(env.validate(), Ok(()));
}

/// 可注入 4xx/5xx：取回层归类 BLOCKED，且不与 PARSE 混用。
#[tokio::test]
async fn http_fixture_injects_error_statuses() {
    let server = Server::start_with(vec![
        ("/missing", Response::status(404)),
        ("/boom", Response::status(500)),
        ("/teapot", Response::status(418)),
    ])
    .await;
    for (path, status) in [("/missing", 404), ("/boom", 500), ("/teapot", 418)] {
        let env = fetch(&server.url(path), &options()).await;
        assert!(!env.ok, "{path}");
        assert_eq!(env.code, ErrorCode::Blocked, "{path}");
        assert_ne!(env.code, ErrorCode::Parse, "{path}");
        assert!(env.error.contains(&status.to_string()), "{}", env.error);
        assert!(env.data.is_none(), "{path}");
    }
}

/// 可注入 429 + Retry-After 与 robots.txt。
#[tokio::test]
async fn http_fixture_injects_retry_after_and_robots() {
    let server = Server::start_with(vec![
        ("/rate-limited", Response::retry_after(2)),
        (
            "/robots.txt",
            Response::robots(&read_fixture_text("http/robots.txt")),
        ),
    ])
    .await;

    let raw = raw_request(
        &server,
        "GET /rate-limited HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 429"), "{raw}");
    assert!(raw.to_ascii_lowercase().contains("retry-after: 2"), "{raw}");

    let raw = raw_request(
        &server,
        "GET /robots.txt HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert!(
        raw.to_ascii_lowercase()
            .contains("content-type: text/plain"),
        "{raw}"
    );
    assert!(raw.contains("Disallow: /private/"), "{raw}");

    let env = fetch(&server.url("/rate-limited"), &options()).await;
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Blocked);
}

/// 可注入大响应：命中上限即截断并带 LIMIT + degraded_from。
#[tokio::test]
async fn http_fixture_injects_large_body_for_truncation() {
    let total = 64 * 1024;
    let server = Server::start_with(vec![("/large", Response::large(total, b'x'))]).await;
    let mut opts = options();
    opts.max_body_bytes = 4096;
    let env = fetch(&server.url("/large"), &opts).await;
    assert!(env.ok);
    assert_eq!(env.code, ErrorCode::Limit);
    assert_eq!(
        env.meta.degraded_from.as_deref(),
        Some("response-body-truncated")
    );
    let data = env.data.as_ref().expect("data");
    assert!(data.content.truncated);
    assert_eq!(data.content.byte_len, 4096);
    assert_eq!(data.content.declared_len, Some(total as u64));
}

/// fixture 的观测能力：方法、body、cookie、并发都可断言。
#[tokio::test]
async fn http_fixture_observes_method_body_cookie_and_concurrency() {
    let server = Server::start_with(vec![(
        "/submit",
        Response::text("ok").delay(Duration::from_millis(120)),
    )])
    .await;

    let raw = raw_request(
        &server,
        "POST /submit HTTP/1.1\r\nhost: localhost\r\ncookie: sid=abc\r\ncontent-length: 11\r\nconnection: close\r\n\r\nhello=world",
    )
    .await;
    assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
    assert_eq!(
        server.request_body("/submit").as_deref(),
        Some("hello=world")
    );
    assert_eq!(
        server.request_header("/submit", "cookie").as_deref(),
        Some("sid=abc")
    );
    let hits = server.hits("/submit");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].method, "POST");

    let url = server.url("/submit");
    let opts = options();
    let f1 = fetch(&url, &opts);
    let f2 = fetch(&url, &opts);
    let f3 = fetch(&url, &opts);
    let f4 = fetch(&url, &opts);
    tokio::join!(f1, f2, f3, f4);
    assert!(
        server.max_in_flight_for("/submit") >= 2,
        "fixture 应观测到并发（实际 {}）",
        server.max_in_flight_for("/submit")
    );
}
