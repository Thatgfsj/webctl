//! 本地 HTTP/1.1 fixture 服务（离线测试专用，不访问外网）。
//!
//! 手写最小服务端：只支持 GET、一连接一请求、Content-Length 响应。
//! 端口用 `127.0.0.1:0` 动态分配，避免并发测试互相踩端口。
#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 一条 fixture 路由。
#[derive(Debug, Clone)]
pub struct Route {
    /// 状态码。
    pub status: u16,
    /// 附加响应头（可重复）。
    pub headers: Vec<(String, String)>,
    /// 响应体字节。
    pub body: Vec<u8>,
    /// 响应前延迟（用于超时用例）。
    pub delay: Duration,
}

impl Route {
    /// 构造一条路由。
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    /// 200 响应。
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self::new(200, body)
    }

    /// 200 `text/html; charset=utf-8` 响应。
    pub fn html(body: impl Into<Vec<u8>>) -> Self {
        Self::ok(body).header("content-type", "text/html; charset=utf-8")
    }

    /// 追加响应头。
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// 设置响应前延迟。
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// 重定向响应。
    pub fn redirect(status: u16, location: impl Into<String>) -> Self {
        let location = location.into();
        Self::new(status, Vec::<u8>::new()).header("location", &location)
    }
}

#[derive(Default)]
struct State {
    routes: Mutex<HashMap<String, Route>>,
    counts: Mutex<HashMap<String, usize>>,
    cookies: Mutex<HashMap<String, String>>,
    total: AtomicUsize,
}

/// 测试服务器句柄；drop 时停止接收连接。
pub struct Server {
    addr: SocketAddr,
    state: Arc<State>,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl Server {
    /// 启动服务器并注册路由。
    pub async fn start<I, P>(routes: I) -> Self
    where
        I: IntoIterator<Item = (P, Route)>,
        P: Into<String>,
    {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture server");
        let addr = listener.local_addr().expect("fixture addr");
        let state = Arc::new(State::default());
        {
            let mut map = state.routes.lock().expect("routes");
            for (path, route) in routes {
                map.insert(path.into(), route);
            }
        }
        let accept_state = Arc::clone(&state);
        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let connection_state = Arc::clone(&accept_state);
                tokio::spawn(async move {
                    handle_connection(stream, connection_state).await;
                });
            }
        });
        Self {
            addr,
            state,
            handle,
        }
    }

    /// `http://127.0.0.1:<port>`。
    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// 拼接绝对 URL。
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    /// 某路径收到的请求数。
    pub fn count(&self, path: &str) -> usize {
        self.state
            .counts
            .lock()
            .expect("counts")
            .get(path)
            .copied()
            .unwrap_or(0)
    }

    /// 收到的请求总数。
    pub fn total(&self) -> usize {
        self.state.total.load(Ordering::SeqCst)
    }

    /// 某路径最近一次收到的 `Cookie` 头。
    pub fn cookie(&self, path: &str) -> Option<String> {
        self.state
            .cookies
            .lock()
            .expect("cookies")
            .get(path)
            .cloned()
    }
}

async fn handle_connection(mut stream: tokio::net::TcpStream, state: Arc<State>) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) => return,
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => return,
        }
        if buffer.len() > 64 * 1024 {
            break;
        }
    }

    let head = String::from_utf8_lossy(&buffer);
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/")
        .to_owned();
    let mut cookie = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("cookie")
        {
            cookie = Some(value.trim().to_owned());
        }
    }

    state.total.fetch_add(1, Ordering::SeqCst);
    *state
        .counts
        .lock()
        .expect("counts")
        .entry(path.clone())
        .or_insert(0) += 1;
    if let Some(cookie) = cookie {
        state
            .cookies
            .lock()
            .expect("cookies")
            .insert(path.clone(), cookie);
    }
    let route = state
        .routes
        .lock()
        .expect("routes")
        .get(&path)
        .cloned()
        .unwrap_or_else(|| Route::new(404, "fixture: no route"));
    if !route.delay.is_zero() {
        tokio::time::sleep(route.delay).await;
    }

    let response = render(&route);
    let _ = stream.write_all(&response).await;
    let _ = stream.shutdown().await;
}

fn render(route: &Route) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {} {}\r\n", route.status, reason(route.status));
    let mut has_content_length = false;
    for (name, value) in &route.headers {
        if name.eq_ignore_ascii_case("content-length") {
            has_content_length = true;
        }
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    if !has_content_length {
        head.push_str("content-length: ");
        head.push_str(&route.body.len().to_string());
        head.push_str("\r\n");
    }
    head.push_str("connection: close\r\n\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(&route.body);
    bytes
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Status",
    }
}
