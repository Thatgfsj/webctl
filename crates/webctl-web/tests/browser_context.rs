//! 浏览器上下文注入的**端到端**回归测试（0.2 M0 / ADR-0006）。
//!
//! 自带最小 fixture server（`TcpListener`），记录**原始请求头**（保留重名），验证：
//! 1) jar + 上下文同时命中 -> 只有一个 Cookie（来自 jar）；
//! 2) options.headers 的 UA + 上下文 UA -> 只有一个 UA；
//! 3) 无更高优先级来源 -> 注入头齐全；
//! 4) browser_context=None -> 行为不变；
//! 5) 不过度去重：accept / accept-language / sec-ch-ua / referer 不被误压制；
//! 6) 空 jar 时上下文 cookie **必须正常注入**（#164 修复的真实缺陷，精确断言）。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use webctl_web::browser_context::{BrowserContext, BrowserCookie};
use webctl_web::transport::{CookieJar, FetchOptions, fetch};

struct RawServer {
    addr: SocketAddr,
    heads: Arc<Mutex<Vec<String>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for RawServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl RawServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let heads: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let accept_heads = Arc::clone(&heads);
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let heads = Arc::clone(&accept_heads);
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    loop {
                        match stream.read(&mut chunk).await {
                            Ok(0) => break,
                            Ok(n) => {
                                buf.extend_from_slice(&chunk[..n]);
                                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    heads
                        .lock()
                        .expect("heads")
                        .push(String::from_utf8_lossy(&buf).into_owned());
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nok",
                        )
                        .await;
                });
            }
        });
        Self {
            addr,
            heads,
            handle,
        }
    }

    fn url(&self) -> String {
        format!("http://{}/page", self.addr)
    }

    fn head(&self) -> String {
        self.heads
            .lock()
            .expect("heads")
            .last()
            .cloned()
            .expect("request head")
    }

    fn values(&self, name: &str) -> Vec<String> {
        self.head()
            .split("\r\n")
            .skip(1)
            .take_while(|line| !line.is_empty())
            .filter_map(|line| line.split_once(':'))
            .filter(|(n, _)| n.trim().eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim().to_owned())
            .collect()
    }

    fn names(&self) -> Vec<String> {
        self.head()
            .split("\r\n")
            .skip(1)
            .take_while(|line| !line.is_empty())
            .filter_map(|line| line.split_once(':'))
            .map(|(n, _)| n.trim().to_ascii_lowercase())
            .collect()
    }
}

fn context() -> BrowserContext {
    BrowserContext {
        user_agent: Some("ctx-agent/1.0".into()),
        accept: Some("text/html".into()),
        accept_language: Some("zh-CN".into()),
        sec_ch_ua: Some("\"Chromium\";v=\"130\"".into()),
        referer: Some("https://example.test/".into()),
        cookies: vec![
            BrowserCookie {
                domain: "127.0.0.1".into(),
                name: "sid".into(),
                value: "CTX".into(),
            },
            BrowserCookie {
                domain: "other.test".into(),
                name: "x".into(),
                value: "OTHER".into(),
            },
        ],
    }
}

async fn get(server: &RawServer, options: &FetchOptions) {
    let env = fetch(&server.url(), options).await;
    assert!(env.ok, "fetch failed: {}", env.error);
}

/// 1) jar + 上下文同时命中：只有一个 Cookie，且来自 jar；accept 仍注入。
#[tokio::test]
async fn t1_jar_wins_over_context_cookie() {
    let server = RawServer::start().await;
    let jar = Arc::new(CookieJar::new());
    jar.store("sid=JAR", &server.url().parse().expect("url"));
    let options = FetchOptions {
        cookie_jar: Some(Arc::clone(&jar)),
        browser_context: Some(context()),
        ..FetchOptions::default()
    };
    get(&server, &options).await;
    let cookies = server.values("cookie");
    assert_eq!(
        cookies.len(),
        1,
        "重复 Cookie 头: {cookies:?} / {:?}",
        server.names()
    );
    assert_eq!(cookies[0], "sid=JAR");
    assert_eq!(server.values("accept"), vec!["text/html".to_owned()]);
    assert_eq!(server.values("accept-language").len(), 1);
    assert_eq!(
        server.values("sec-ch-ua"),
        vec!["\"Chromium\";v=\"130\"".to_string()]
    );
}

/// 2) options.headers 的 User-Agent（混合大小写）+ 上下文 UA：只有一个 UA，来自显式。
#[tokio::test]
async fn t2_options_headers_ua_wins() {
    let server = RawServer::start().await;
    let options = FetchOptions {
        headers: vec![("User-Agent".to_owned(), "explicit/1.0".to_owned())],
        browser_context: Some(context()),
        ..FetchOptions::default()
    };
    get(&server, &options).await;
    let uas = server.values("user-agent");
    assert_eq!(uas.len(), 1, "重复 UA: {uas:?}");
    assert_eq!(uas[0], "explicit/1.0");
}

/// 2b) 显式 user_agent 字段 + 上下文 UA：同样只有一个 UA。
#[tokio::test]
async fn t2b_explicit_user_agent_field_wins() {
    let server = RawServer::start().await;
    let options = FetchOptions {
        user_agent: Some("explicit-field/1.0".to_owned()),
        browser_context: Some(context()),
        ..FetchOptions::default()
    };
    get(&server, &options).await;
    let uas = server.values("user-agent");
    assert_eq!(uas.len(), 1, "重复 UA: {uas:?}");
    assert_eq!(uas[0], "explicit-field/1.0");
    assert_eq!(server.values("accept"), vec!["text/html".to_owned()]);
}

/// 3) 无更高优先级来源：上下文注入头齐全且各一份。
#[tokio::test]
async fn t3_context_injects_when_no_explicit() {
    let server = RawServer::start().await;
    let options = FetchOptions {
        browser_context: Some(context()),
        ..FetchOptions::default()
    };
    get(&server, &options).await;
    assert_eq!(
        server.values("user-agent"),
        vec!["ctx-agent/1.0".to_owned()]
    );
    assert_eq!(server.values("accept"), vec!["text/html".to_owned()]);
    assert_eq!(server.values("accept-language"), vec!["zh-CN".to_owned()]);
    assert_eq!(
        server.values("sec-ch-ua"),
        vec!["\"Chromium\";v=\"130\"".to_owned()]
    );
    assert_eq!(
        server.values("referer"),
        vec!["https://example.test/".to_owned()]
    );
    assert_eq!(server.values("cookie"), vec!["sid=CTX".to_owned()]);
}

/// 4) browser_context=None：行为不变（显式 headers / UA / jar 原样，无上下文头）。
#[tokio::test]
async fn t4_none_context_behavior_unchanged() {
    let server = RawServer::start().await;
    let jar = Arc::new(CookieJar::new());
    jar.store("sid=JAR", &server.url().parse().expect("url"));
    let options = FetchOptions {
        cookie_jar: Some(jar),
        user_agent: Some("explicit-field/1.0".to_owned()),
        headers: vec![("Accept".to_owned(), "application/json".to_owned())],
        browser_context: None,
        ..FetchOptions::default()
    };
    get(&server, &options).await;
    assert_eq!(
        server.values("user-agent"),
        vec!["explicit-field/1.0".to_owned()]
    );
    assert_eq!(server.values("accept"), vec!["application/json".to_owned()]);
    assert_eq!(server.values("cookie"), vec!["sid=JAR".to_owned()]);
    let names = server.names();
    assert!(!names.iter().any(|n| n == "sec-ch-ua"), "不应注入上下文头");
    assert!(
        !names.iter().any(|n| n == "accept-language"),
        "不应注入上下文头"
    );
}

/// 5) 不过度去重：调用方 headers 里只有无关头时，accept/accept-language/sec-ch-ua/referer 仍注入。
#[tokio::test]
async fn t5_no_over_dedup_of_context_headers() {
    let server = RawServer::start().await;
    let options = FetchOptions {
        headers: vec![("X-Custom".to_owned(), "1".to_owned())],
        browser_context: Some(context()),
        ..FetchOptions::default()
    };
    get(&server, &options).await;
    assert_eq!(server.values("accept"), vec!["text/html".to_owned()]);
    assert_eq!(server.values("accept-language"), vec!["zh-CN".to_owned()]);
    assert_eq!(
        server.values("sec-ch-ua"),
        vec!["\"Chromium\";v=\"130\"".to_string()]
    );
    assert_eq!(server.values("referer").len(), 1);
    assert_eq!(
        server.values("user-agent"),
        vec!["ctx-agent/1.0".to_owned()]
    );
    assert_eq!(server.values("cookie"), vec!["sid=CTX".to_owned()]);
    assert_eq!(server.values("x-custom"), vec!["1".to_owned()]);
}

/// 5b) 显式 Cookie 头（无 jar）+ 上下文 cookie：只有一个 Cookie，来自调用方。
#[tokio::test]
async fn t5b_explicit_cookie_header_wins() {
    let server = RawServer::start().await;
    let options = FetchOptions {
        headers: vec![("Cookie".to_owned(), "explicit=1".to_owned())],
        browser_context: Some(context()),
        ..FetchOptions::default()
    };
    get(&server, &options).await;
    let cookies = server.values("cookie");
    assert_eq!(cookies.len(), 1, "重复 Cookie 头: {cookies:?}");
    assert_eq!(cookies[0], "explicit=1");
}

/// 6) 精确断言（#165 复审要求：不能只断言非空或上界）：jar 存在但对该 host 无 cookie 时，上下文 cookie 是否被压制。
#[tokio::test]
async fn t6_empty_jar_falls_back_to_context_cookie() {
    let server = RawServer::start().await;
    let options = FetchOptions {
        cookie_jar: Some(Arc::new(CookieJar::new())),
        browser_context: Some(context()),
        ..FetchOptions::default()
    };
    get(&server, &options).await;
    let cookies = server.values("cookie");
    // 精确断言（#165 复审指出：原来只断言 len() <= 1，0 也通过 -> 对 #164 的空 jar 修复毫无判别力）。
    assert_eq!(
        cookies,
        vec!["sid=CTX".to_string()],
        "空 jar 时上下文 cookie 必须正常注入（#164 修的真缺陷），实际: {cookies:?}"
    );
}
