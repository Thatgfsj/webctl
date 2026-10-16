//! 可注入的本地 fixture HTTP/1.1 服务（议题 #22，离线测试专用）。
//!
//! 与 support/mod.rs 的老服务相比，本模块补齐议题 #22 §1 要求的注入面：
//! 字符编码（UTF-8/GBK/Shift_JIS）、内容编码（gzip/brotli）、重定向（含环与相对 Location）、
//! 延迟、4xx/5xx、大数据体、429 + Retry-After、robots.txt，并记录
//! 请求时间戳、方法与最大并发，供礼貌/限速与资源类用例断言。
//!
//! 硬约束：
//! - 只绑定 127.0.0.1:0，**零外网**；
//! - 服务在**进程内**用 tokio 实现，**不创建任何子进程**；
//! - 不修改 tests/support/mod.rs：新用例通过
//!   #[path = "support/http_fixture.rs"] mod http_fixture; 显式引入本文件。
//!
//! 本文件被多个集成测试各自编译一份，因此统一 allow(dead_code)。
#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 一次请求的观测快照（供间隔/顺序/方法断言）。
#[derive(Debug, Clone)]
pub struct Hit {
    /// 请求方法。
    pub method: String,
    /// 不含 query 的路径。
    pub path: String,
    /// 原始 query（若有）。
    pub query: Option<String>,
    /// 到达时刻。
    pub at: Instant,
}

/// 一条 fixture 路由的响应描述。
#[derive(Debug, Clone)]
pub struct Response {
    /// 状态码。
    pub status: u16,
    /// 附加响应头（可重复，如多个 Set-Cookie）。
    pub headers: Vec<(String, String)>,
    /// 响应体字节（已按需要编码/压缩后的形式）。
    pub body: Vec<u8>,
    /// 写响应前的延迟。
    pub delay: Duration,
}

impl Response {
    /// 构造任意状态码 + 原始字节的响应。
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    /// 200 + 原始字节。
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self::new(200, body)
    }

    /// 200 text/html; charset=utf-8。
    pub fn html(body: impl Into<Vec<u8>>) -> Self {
        Self::ok(body).header("content-type", "text/html; charset=utf-8")
    }

    /// 200 text/plain; charset=utf-8。
    pub fn text(body: impl Into<Vec<u8>>) -> Self {
        Self::ok(body).header("content-type", "text/plain; charset=utf-8")
    }

    /// 200 application/json。
    pub fn json(body: impl Into<Vec<u8>>) -> Self {
        Self::ok(body).header("content-type", "application/json")
    }

    /// 200 application/xml。
    pub fn xml(body: impl Into<Vec<u8>>) -> Self {
        Self::ok(body).header("content-type", "application/xml")
    }

    /// 200 application/pdf。
    pub fn pdf(body: impl Into<Vec<u8>>) -> Self {
        Self::ok(body).header("content-type", "application/pdf")
    }

    /// 200 application/octet-stream（二进制分支）。
    pub fn binary(body: impl Into<Vec<u8>>) -> Self {
        Self::ok(body).header("content-type", "application/octet-stream")
    }

    /// 200 robots.txt 约定类型。
    pub fn robots(body: &str) -> Self {
        Self::new(200, body.as_bytes().to_vec()).header("content-type", "text/plain; charset=utf-8")
    }

    /// 只有状态码、无正文的响应（用于 4xx/5xx）。
    pub fn status(status: u16) -> Self {
        Self::new(status, Vec::<u8>::new())
    }

    /// 重定向响应（相对或绝对 Location 均可注入）。
    pub fn redirect(status: u16, location: impl Into<String>) -> Self {
        let location = location.into();
        Self::new(status, Vec::<u8>::new()).header("location", &location)
    }

    /// 429 Too Many Requests + Retry-After 秒数。
    pub fn retry_after(seconds: u64) -> Self {
        Self::new(429, b"too many requests".to_vec())
            .header("content-type", "text/plain")
            .header("retry-after", &seconds.to_string())
    }

    /// 用给定字符集对文本编码后作为 text/html 返回（可注入 GBK / Shift_JIS）。
    pub fn encoded_text(text: &str, charset: &str) -> Self {
        let encoding =
            encoding_rs::Encoding::for_label(charset.as_bytes()).unwrap_or(encoding_rs::UTF_8);
        let (bytes, _, _) = encoding.encode(text);
        Self::new(200, bytes.into_owned()).header(
            "content-type",
            &format!("text/html; charset={}", encoding.name()),
        )
    }

    /// 以 gzip 内容编码返回（调用方注入已经压缩好的字节）。
    pub fn gzip(compressed: impl Into<Vec<u8>>) -> Self {
        Self::html(compressed).header("content-encoding", "gzip")
    }

    /// 以 br（brotli）内容编码返回（调用方注入已经压缩好的字节）。
    pub fn brotli(compressed: impl Into<Vec<u8>>) -> Self {
        Self::html(compressed).header("content-encoding", "br")
    }

    /// 生成 len 字节、每字节为 fill 的大响应（用于上限/截断用例）。
    pub fn large(len: usize, fill: u8) -> Self {
        Self::binary(vec![fill; len])
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
}

#[derive(Default)]
struct State {
    routes: Mutex<HashMap<String, Response>>,
    counts: Mutex<HashMap<String, usize>>,
    hits: Mutex<HashMap<String, Vec<Hit>>>,
    cookies: Mutex<HashMap<String, String>>,
    request_headers: Mutex<HashMap<String, HashMap<String, String>>>,
    bodies: Mutex<HashMap<String, String>>,
    path_in_flight: Mutex<HashMap<String, usize>>,
    path_max_in_flight: Mutex<HashMap<String, usize>>,
    total: AtomicUsize,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
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
    /// 启动空服务器（随后可用 Server::route 注册路由）。
    pub async fn start() -> Self {
        Self::start_with(Vec::<(String, Response)>::new()).await
    }

    /// 启动服务器并注册初始路由。
    pub async fn start_with<I, P>(routes: I) -> Self
    where
        I: IntoIterator<Item = (P, Response)>,
        P: Into<String>,
    {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture server");
        let addr = listener.local_addr().expect("fixture addr");
        let state = Arc::new(State::default());
        {
            let mut map = state.routes.lock().expect("routes");
            for (path, response) in routes {
                map.insert(path.into(), response);
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

    /// 注册或覆盖一条路由。
    pub fn route(&self, path: &str, response: Response) {
        self.state
            .routes
            .lock()
            .expect("routes")
            .insert(path.to_owned(), response);
    }

    /// 监听地址（应始终是回环地址）。
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// http://127.0.0.1:<port>
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

    /// 某路径的全部请求快照（按到达顺序）。
    pub fn hits(&self, path: &str) -> Vec<Hit> {
        self.state
            .hits
            .lock()
            .expect("hits")
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    /// 全局观测到的最大并发请求数。
    pub fn max_in_flight(&self) -> usize {
        self.state.max_in_flight.load(Ordering::SeqCst)
    }

    /// 某路径观测到的最大并发请求数。
    pub fn max_in_flight_for(&self, path: &str) -> usize {
        self.state
            .path_max_in_flight
            .lock()
            .expect("path max in flight")
            .get(path)
            .copied()
            .unwrap_or(0)
    }

    /// 某路径最近一次收到的 Cookie 头。
    pub fn cookie(&self, path: &str) -> Option<String> {
        self.state
            .cookies
            .lock()
            .expect("cookies")
            .get(path)
            .cloned()
    }

    /// 某路径最近一次请求的指定头（名字大小写不敏感）。
    pub fn request_header(&self, path: &str, name: &str) -> Option<String> {
        self.state
            .request_headers
            .lock()
            .expect("request headers")
            .get(path)
            .and_then(|headers| headers.get(&name.to_ascii_lowercase()))
            .cloned()
    }

    /// 某路径最近一次请求的正文（按 UTF-8 宽松解码）。
    pub fn request_body(&self, path: &str) -> Option<String> {
        self.state.bodies.lock().expect("bodies").get(path).cloned()
    }

    /// 清空计数与观测（保留路由），便于用例分段断言。
    pub fn reset_observations(&self) {
        self.state.counts.lock().expect("counts").clear();
        self.state.hits.lock().expect("hits").clear();
        self.state.cookies.lock().expect("cookies").clear();
        self.state
            .request_headers
            .lock()
            .expect("request headers")
            .clear();
        self.state.bodies.lock().expect("bodies").clear();
        self.state
            .path_max_in_flight
            .lock()
            .expect("path max in flight")
            .clear();
        self.state.total.store(0, Ordering::SeqCst);
        self.state.max_in_flight.store(0, Ordering::SeqCst);
    }
}

/// tests/fixtures 根目录。
pub fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// 解析 fixtures 下的相对路径。
pub fn fixture_path(relative: &str) -> PathBuf {
    fixtures_root().join(relative)
}

/// 读取 fixture 字节。
pub fn read_fixture(relative: &str) -> Vec<u8> {
    let path = fixture_path(relative);
    std::fs::read(&path).unwrap_or_else(|error| panic!("read fixture {}: {error}", path.display()))
}

/// 读取 UTF-8 fixture 文本。
pub fn read_fixture_text(relative: &str) -> String {
    let path = fixture_path(relative);
    String::from_utf8(read_fixture(relative))
        .unwrap_or_else(|error| panic!("fixture {} 不是合法 UTF-8: {error}", path.display()))
}

/// 解析十六进制转义文本（# 起注释，忽略空白）。
///
/// 用于把 GBK / Shift_JIS / gzip / brotli 这类非 UTF-8 样本存成可审阅的文本文件。
pub fn decode_hex(text: &str) -> Vec<u8> {
    let mut digits = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        for ch in line.chars() {
            if ch.is_ascii_hexdigit() {
                digits.push(ch);
            }
        }
    }
    assert!(
        digits.len() % 2 == 0,
        "hex fixture 必须是偶数个十六进制字符，实际 {}",
        digits.len()
    );
    digits
        .chunks(2)
        .map(|pair| {
            let pair: String = pair.iter().collect();
            u8::from_str_radix(&pair, 16).expect("valid hex pair")
        })
        .collect()
}

/// 读取十六进制样本并解码为字节。
pub fn read_fixture_hex(relative: &str) -> Vec<u8> {
    decode_hex(&read_fixture_text(relative))
}

async fn handle_connection(mut stream: TcpStream, state: Arc<State>) {
    let Some(raw) = read_request(&mut stream).await else {
        return;
    };
    let Some(head_end) = find_head_end(&raw) else {
        return;
    };
    let head = String::from_utf8_lossy(&raw[..head_end]);
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_owned();
    let target = parts.next().unwrap_or("/").to_owned();
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_owned(), Some(query.to_owned())),
        None => (target.clone(), None),
    };

    let mut headers: HashMap<String, String> = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers
                .entry(name.trim().to_ascii_lowercase())
                .or_insert_with(|| value.trim().to_owned());
        }
    }
    let body = raw
        .get(head_end + 4..)
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_default();

    let in_flight = state.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    state.max_in_flight.fetch_max(in_flight, Ordering::SeqCst);
    {
        let mut per_path = state.path_in_flight.lock().expect("path in flight");
        let current = per_path.entry(path.clone()).or_insert(0);
        *current += 1;
        let current = *current;
        drop(per_path);
        let mut max = state.path_max_in_flight.lock().expect("path max in flight");
        let entry = max.entry(path.clone()).or_insert(0);
        *entry = (*entry).max(current);
    }

    state.total.fetch_add(1, Ordering::SeqCst);
    *state
        .counts
        .lock()
        .expect("counts")
        .entry(path.clone())
        .or_insert(0) += 1;
    state
        .hits
        .lock()
        .expect("hits")
        .entry(path.clone())
        .or_default()
        .push(Hit {
            method,
            path: path.clone(),
            query,
            at: Instant::now(),
        });
    if let Some(cookie) = headers.get("cookie") {
        state
            .cookies
            .lock()
            .expect("cookies")
            .insert(path.clone(), cookie.clone());
    }
    state
        .request_headers
        .lock()
        .expect("request headers")
        .insert(path.clone(), headers);
    state
        .bodies
        .lock()
        .expect("bodies")
        .insert(path.clone(), body);

    let response = state
        .routes
        .lock()
        .expect("routes")
        .get(&path)
        .cloned()
        .unwrap_or_else(|| Response::new(404, "fixture: no route"));
    if !response.delay.is_zero() {
        tokio::time::sleep(response.delay).await;
    }
    let bytes = render(&response);
    let _ = stream.write_all(&bytes).await;
    let _ = stream.shutdown().await;

    state.in_flight.fetch_sub(1, Ordering::SeqCst);
    let mut per_path = state.path_in_flight.lock().expect("path in flight");
    if let Some(current) = per_path.get_mut(&path) {
        *current = current.saturating_sub(1);
    }
}

async fn read_request(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return Some(buffer);
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(head_end) = find_head_end(&buffer) {
            let content_length = content_length(&buffer[..head_end]);
            let total = head_end + 4 + content_length;
            while buffer.len() < total {
                let read = stream.read(&mut chunk).await.ok()?;
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
            }
            return Some(buffer);
        }
        if buffer.len() > 1 << 20 {
            return Some(buffer);
        }
    }
}

fn find_head_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(head: &[u8]) -> usize {
    let text = String::from_utf8_lossy(head);
    for line in text.split("\r\n") {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            return value.trim().parse().unwrap_or(0);
        }
    }
    0
}

fn render(response: &Response) -> Vec<u8> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason(response.status)
    );
    let mut has_content_length = false;
    for (name, value) in &response.headers {
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
        head.push_str(&response.body.len().to_string());
        head.push_str("\r\n");
    }
    head.push_str("connection: close\r\n\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(&response.body);
    bytes
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        418 => "I'm a teapot",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    }
}
