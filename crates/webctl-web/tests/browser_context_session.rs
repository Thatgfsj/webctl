//! #183：会话路径（GET / 表单 POST）也注入宿主浏览器上下文。
//!
//! 此前 `SessionEngine::send` 自己拼头：只加了 options 的 user_agent、jar 的 cookie 与显式
//! headers，**没有**用 `inject_headers`；同一份上下文经 MCP fetch 生效、经表单 POST 不生效。
//! 本文件用原始 socket fixture 记录请求头，逐条钉住注入与去重规则。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use webctl_web::browser_context::{BrowserContext, BrowserCookie};
use webctl_web::session::{FormFill, SessionEngine, SessionStore};

const FORM_PAGE: &str = "<!doctype html><html><body><form method=\"post\" action=\"/submit\"><input type=\"text\" name=\"q\" value=\"a\"></form></body></html>";

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
        let recorded = Arc::clone(&heads);
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let recorded = Arc::clone(&recorded);
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
                    let head = String::from_utf8_lossy(&buf).into_owned();
                    let is_post = head.starts_with("POST ");
                    recorded.lock().expect("heads").push(head);
                    let body = if is_post { "ok" } else { FORM_PAGE };
                    let content_type = if is_post { "text/plain" } else { "text/html" };
                    // 只在 POST 响应里种 jar cookie：POST 前的 GET 不得让 jar 抢走上下文 cookie，
                    // 这样才能分别钉住"注入"与"去重"两种语义。
                    let set_cookie = if is_post {
                        "Set-Cookie: jar=1; Path=/\r\n"
                    } else {
                        ""
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n{set_cookie}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
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

    fn head(&self, index: usize) -> String {
        let heads = self.heads.lock().expect("heads");
        heads
            .get(index)
            .cloned()
            .unwrap_or_else(|| panic!("缺少第 {index} 个请求"))
    }

    fn len(&self) -> usize {
        self.heads.lock().expect("heads").len()
    }
}

fn context() -> BrowserContext {
    BrowserContext {
        user_agent: Some("ctx-agent/2.0".into()),
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

fn engine(tag: &str, context: Option<BrowserContext>) -> SessionEngine {
    let dir = std::env::temp_dir().join(format!("webctl-ctx-session-{tag}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    SessionEngine::with_store(tag, SessionStore::new(dir))
        .expect("engine")
        .with_browser_context(context)
}

fn header_count(head: &str, name: &str) -> usize {
    head.split("\r\n")
        .skip(1)
        .take_while(|line| !line.is_empty())
        .filter(|line| {
            line.split_once(':')
                .is_some_and(|(n, _)| n.trim().eq_ignore_ascii_case(name))
        })
        .count()
}

fn has_header(head: &str, name: &str, value: &str) -> bool {
    head.split("\r\n")
        .skip(1)
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_once(':'))
        .any(|(n, v)| n.trim().eq_ignore_ascii_case(name) && v.trim().eq_ignore_ascii_case(value))
}

/// 会话 GET：上下文头必须注入（GET 走 transport::fetch，此前已注入；这条是回归守卫）。
#[tokio::test]
async fn session_get_injects_context_headers() {
    let server = RawServer::start().await;
    let engine = engine("ctx-get", Some(context()));
    let page = engine.fetch_page(&server.url()).await;
    assert!(page.ok, "取页失败：{}", page.error);
    let head = server.head(0);
    assert!(
        has_header(&head, "user-agent", "ctx-agent/2.0"),
        "缺少 UA：{head}"
    );
    assert!(
        has_header(&head, "accept", "text/html"),
        "缺少 accept：{head}"
    );
    assert!(
        has_header(&head, "accept-language", "zh-CN"),
        "缺少 accept-language：{head}"
    );
    assert!(
        has_header(&head, "referer", "https://example.test/"),
        "缺少 referer：{head}"
    );
    // 上下文 cookie 只按域名发送：127.0.0.1 的 sid，other.test 的不出现。
    assert!(head.contains("sid=CTX"), "缺少上下文 cookie：{head}");
    assert!(!head.contains("x=OTHER"), "跨域 cookie 被误发：{head}");
}

/// 表单 POST：同样注入（此前 submit_form 走的 send 完全不带上下文）。
#[tokio::test]
async fn session_post_injects_context_headers() {
    let server = RawServer::start().await;
    let engine = engine("ctx-post", Some(context()));
    let page = engine.fetch_page(&server.url()).await.data.expect("page");
    assert!(!page.forms.is_empty(), "fixture 应提供表单");
    let submitted = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(submitted.ok, "提交失败：{}", submitted.error);
    assert!(server.len() >= 2, "应同时有 GET 与 POST 两个请求");
    let post = server.head(1);
    assert!(post.starts_with("POST "), "第二个请求应是 POST：{post}");
    assert!(
        has_header(&post, "user-agent", "ctx-agent/2.0"),
        "POST 缺少 UA：{post}"
    );
    assert!(
        has_header(&post, "accept-language", "zh-CN"),
        "POST 缺少 accept-language：{post}"
    );
    assert!(post.contains("sid=CTX"), "POST 缺少上下文 cookie：{post}");
}

/// 去重：jar 已发 Cookie 时不再发上下文 Cookie（不得出现两个 Cookie 行）。
#[tokio::test]
async fn jar_cookie_wins_over_context_cookie() {
    let server = RawServer::start().await;
    let engine = engine("ctx-dedupe", Some(context()));
    // fixture 只在 POST 响应里 Set-Cookie: jar=1：先 GET 拿表单，POST 一次把 cookie 种进 jar，
    // 再 GET 时 jar 会发 Cookie —— 此时上下文 cookie 必须让位（不得出现两个 Cookie 行）。
    let page = engine.fetch_page(&server.url()).await.data.expect("page");
    let submitted = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(submitted.ok, "{}", submitted.error);
    let after = engine.fetch_page(&server.url()).await;
    assert!(after.ok, "{}", after.error);
    assert!(server.len() >= 3, "应有 GET / POST / GET 三个请求");
    let head = server.head(2);
    assert_eq!(
        header_count(&head, "cookie"),
        1,
        "只允许一个 Cookie 行：{head}"
    );
    assert!(head.contains("jar=1"), "jar cookie 应优先：{head}");
    assert_eq!(
        header_count(&head, "user-agent"),
        1,
        "只允许一个 UA：{head}"
    );
}

/// 反向：没有上下文时行为不变（不注入任何额外头）。
#[tokio::test]
async fn without_context_nothing_is_injected() {
    let server = RawServer::start().await;
    let engine = engine("ctx-none", None);
    let page = engine.fetch_page(&server.url()).await;
    assert!(page.ok, "{}", page.error);
    let head = server.head(0);
    assert!(!head.contains("ctx-agent/2.0"), "未注入 UA：{head}");
    assert!(!head.contains("sid=CTX"), "未注入 cookie：{head}");
}
