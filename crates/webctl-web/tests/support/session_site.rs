//! 本地表单站 fixture（手写最小 HTTP/1.1：GET/POST/cookie/重定向/multipart）。
//!
//! 由 tests/session.rs 拆到 tests/support/，供 session 与 session_forms 两个测试二进制共用；
//! 用 #[path] 引入时编译器不会把它当成独立测试目标。

// 共享 fixture：每个测试二进制只用自己需要的那部分，未用到的助手不算死代码。
#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn cookie(&self, name: &str) -> Option<String> {
        let raw = self.header("cookie")?;
        for pair in raw.split(';') {
            if let Some((key, value)) = pair.trim().split_once('=')
                && key == name
            {
                return Some(value.to_owned());
            }
        }
        None
    }

    pub fn form(&self) -> HashMap<String, String> {
        parse_urlencoded(&self.body)
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn html(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: vec![(
                "content-type".to_owned(),
                "text/html; charset=utf-8".to_owned(),
            )],
            body: body.into(),
        }
    }

    pub fn redirect(location: &str) -> Self {
        Self::redirect_with(302, location)
    }

    /// 指定状态码的重定向（303/307/308 语义用例）。
    pub fn redirect_with(status: u16, location: &str) -> Self {
        Self {
            status,
            headers: vec![("location".to_owned(), location.to_owned())],
            body: Vec::new(),
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_owned(), value.into()));
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct Part {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub content: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct State {
    pub tokens: HashMap<String, String>,
    pub used_tokens: BTreeSet<String>,
    pub sessions: HashMap<String, String>,
    pub session_seq: usize,
    pub token_seq: usize,
    pub recorded: HashMap<String, HashMap<String, String>>,
    pub upload: Option<(String, Vec<u8>, Option<String>)>,
    /// 最近一次请求的原始 body（按路径）。
    pub bodies: HashMap<String, String>,
    /// 最近一次请求的方法（按路径）。
    pub methods: HashMap<String, String>,
}

impl State {
    pub fn issue(&mut self, form: &str) -> String {
        self.token_seq += 1;
        let token = format!("tok-{}", self.token_seq);
        self.tokens.insert(form.to_owned(), token.clone());
        token
    }

    pub fn take(&mut self, form: &str, provided: Option<&String>) -> bool {
        let Some(current) = self.tokens.get(form) else {
            return false;
        };
        if provided != Some(current) {
            return false;
        }
        let token = current.clone();
        self.tokens.remove(form);
        self.used_tokens.insert(token);
        true
    }

    pub fn new_session(&mut self, user: &str) -> String {
        self.session_seq += 1;
        let sid = format!("sid-{}", self.session_seq);
        self.sessions.insert(sid.clone(), user.to_owned());
        sid
    }

    pub fn user_for(&self, sid: &str) -> Option<&str> {
        self.sessions.get(sid).map(String::as_str)
    }

    pub fn record(&mut self, path: &str, fields: HashMap<String, String>) {
        self.recorded.insert(path.to_owned(), fields);
    }

    pub fn recorded(&self, path: &str) -> HashMap<String, String> {
        self.recorded.get(path).cloned().unwrap_or_default()
    }

    /// 记录一次请求的方法与原始 body（用于重定向与同名多值断言）。
    pub fn record_echo(&mut self, path: &str, method: &str, body: &[u8]) {
        self.methods.insert(path.to_owned(), method.to_owned());
        self.bodies
            .insert(path.to_owned(), String::from_utf8_lossy(body).into_owned());
    }
}

pub struct Site {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    hits: Arc<Mutex<HashMap<String, usize>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for Site {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl Site {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind site");
        let addr = listener.local_addr().expect("site addr");
        let state = Arc::new(Mutex::new(State::default()));
        let hits = Arc::new(Mutex::new(HashMap::new()));
        let accept_state = Arc::clone(&state);
        let accept_hits = Arc::clone(&hits);
        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let state = Arc::clone(&accept_state);
                let hits = Arc::clone(&accept_hits);
                tokio::spawn(async move {
                    let _ = serve(stream, state, hits).await;
                });
            }
        });
        Self {
            addr,
            state,
            hits,
            handle,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    pub fn hits(&self, path: &str) -> usize {
        self.hits
            .lock()
            .expect("hits")
            .get(path)
            .copied()
            .unwrap_or(0)
    }

    pub fn with_state<R>(&self, f: impl FnOnce(&State) -> R) -> R {
        let guard = self.state.lock().expect("state");
        f(&guard)
    }

    /// 某路径最近一次请求的方法。
    pub fn method(&self, path: &str) -> Option<String> {
        self.with_state(|state| state.methods.get(path).cloned())
    }

    /// 某路径最近一次请求的原始 body。
    pub fn body(&self, path: &str) -> Option<String> {
        self.with_state(|state| state.bodies.get(path).cloned())
    }
}

async fn serve(
    mut stream: TcpStream,
    state: Arc<Mutex<State>>,
    hits: Arc<Mutex<HashMap<String, usize>>>,
) -> std::io::Result<()> {
    let Some(request) = read_request(&mut stream).await else {
        return Ok(());
    };
    *hits
        .lock()
        .expect("hits")
        .entry(request.path.clone())
        .or_insert(0) += 1;
    let response = {
        let mut guard = state.lock().expect("state");
        handle(&request, &mut guard)
    };
    let bytes = render(&response);
    stream.write_all(&bytes).await?;
    stream.shutdown().await
}

async fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_double_crlf(&buffer) {
            let head = String::from_utf8_lossy(&buffer[..position]).to_string();
            let content_length = content_length(&head);
            let mut body = buffer[position + 4..].to_vec();
            while body.len() < content_length {
                let read = stream.read(&mut chunk).await.ok()?;
                if read == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..read]);
            }
            body.truncate(content_length);
            return Some(parse_request(&head, body));
        }
        if buffer.len() > 1 << 20 {
            return None;
        }
    }
}

fn find_double_crlf(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(head: &str) -> usize {
    for line in head.split("\r\n") {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            return value.trim().parse().unwrap_or(0);
        }
    }
    0
}

fn parse_request(head: &str, body: Vec<u8>) -> Request {
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or("/").to_owned();
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_owned(), parse_query(query)),
        None => (target, HashMap::new()),
    };
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    Request {
        method,
        path,
        query,
        headers,
        body,
    }
}

fn parse_query(query: &str) -> HashMap<String, String> {
    parse_pairs(query.split('&'))
}

pub fn parse_urlencoded(body: &[u8]) -> HashMap<String, String> {
    let text = String::from_utf8_lossy(body);
    parse_pairs(text.split('&'))
}

/// 保序解析 urlencoded（同名多值断言用，HashMap 会丢序/丢重复）。
pub fn parse_pairs_ordered(body: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(body);
    text.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

fn parse_pairs<'a>(pairs: impl Iterator<Item = &'a str>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in pairs {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(percent_decode(key), percent_decode(value));
    }
    map
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("00");
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    index += 3;
                } else {
                    out.push(bytes[index]);
                    index += 1;
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_multipart(body: &[u8], boundary: &str) -> Vec<Part> {
    let text = String::from_utf8_lossy(body).into_owned();
    let delimiter = format!("--{boundary}");
    let mut parts = Vec::new();
    for chunk in text.split(&delimiter) {
        let chunk = chunk.strip_prefix("\r\n").unwrap_or(chunk);
        if chunk.is_empty() || chunk.starts_with("--") {
            continue;
        }
        let Some(header_end) = chunk.find("\r\n\r\n") else {
            continue;
        };
        let head = &chunk[..header_end];
        let content = chunk[header_end + 4..]
            .strip_suffix("\r\n")
            .unwrap_or(&chunk[header_end + 4..]);
        let mut part = Part {
            name: String::new(),
            filename: None,
            content_type: None,
            content: content.as_bytes().to_vec(),
        };
        for line in head.split("\r\n") {
            if let Some((name, value)) = line.split_once(':') {
                let name = name.trim().to_ascii_lowercase();
                let value = value.trim();
                if name == "content-disposition" {
                    for token in value.split(';') {
                        let token = token.trim();
                        if let Some(rest) = token.strip_prefix("name=") {
                            part.name = rest.trim_matches('"').to_owned();
                        }
                        if let Some(rest) = token.strip_prefix("filename=") {
                            part.filename = Some(rest.trim_matches('"').to_owned());
                        }
                    }
                } else if name == "content-type" {
                    part.content_type = Some(value.to_owned());
                }
            }
        }
        parts.push(part);
    }
    parts
}

fn render(response: &Response) -> Vec<u8> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason(response.status)
    );
    let mut has_length = false;
    for (name, value) in &response.headers {
        if name.eq_ignore_ascii_case("content-length") {
            has_length = true;
        }
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    if !has_length {
        head.push_str(&format!("content-length: {}\r\n", response.body.len()));
    }
    head.push_str("connection: close\r\n\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(&response.body);
    bytes
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        302 => "Found",
        303 => "See Other",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Status",
    }
}

// ---- 路由处理 ----

pub fn handle(request: &Request, state: &mut State) -> Response {
    if let Some(rest) = request.path.strip_prefix("/echo/") {
        state.record_echo(&request.path, &request.method, &request.body);
        return Response::html(200, format!("<html><body>echo {rest}</body></html>"));
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/login") => {
            let token = state.issue("login");
            Response::html(200, login_page(&token, ""))
        }
        ("POST", "/login") => {
            let fields = request.form();
            if !state.take("login", fields.get("csrf_token")) {
                return Response::html(403, "<html><body>bad csrf</body></html>");
            }
            if fields.get("username").map(String::as_str) == Some("alice")
                && fields.get("password").map(String::as_str) == Some("secret")
            {
                let sid = state.new_session("alice");
                Response::redirect("/dashboard")
                    .header("set-cookie", format!("sid={sid}; Path=/; HttpOnly"))
            } else {
                let token = state.issue("login");
                Response::html(200, login_page(&token, "invalid credentials"))
            }
        }
        ("GET", "/dashboard") => match request
            .cookie("sid")
            .as_deref()
            .and_then(|sid| state.user_for(sid))
        {
            Some(user) => Response::html(
                200,
                format!("<html><body><h1>Welcome {user}</h1></body></html>"),
            ),
            None => Response::redirect("/login"),
        },
        ("GET", "/protected") => Response::html(200, "<html><body>top secret</body></html>"),
        ("GET", "/form/basic") => {
            let token = state.issue("basic");
            Response::html(200, basic_form(&token))
        }
        ("POST", "/form/landing") => {
            let fields = request.form();
            if !state.take("basic", fields.get("csrf_token")) {
                return Response::html(403, "<html><body>bad csrf</body></html>");
            }
            state.record("/form/landing", fields);
            Response::html(200, "<html><body>landed</body></html>")
        }
        ("GET", "/form/required") => {
            let token = state.issue("required");
            Response::html(
                200,
                format!(
                    "<form action=\"/form/required\" method=\"post\">\
                     <input type=\"hidden\" name=\"csrf_token\" value=\"{token}\">\
                     <input type=\"text\" name=\"must\" required>\
                     <input type=\"submit\" value=\"Go\"></form>"
                ),
            )
        }
        ("POST", "/form/required") => {
            let fields = request.form();
            if !state.take("required", fields.get("csrf_token")) {
                return Response::html(403, "<html><body>bad csrf</body></html>");
            }
            state.record("/form/required", fields);
            Response::html(200, "<html><body>required-ok</body></html>")
        }
        ("GET", "/form/upload") => {
            let token = state.issue("upload");
            Response::html(
                200,
                format!(
                    "<form action=\"/form/upload\" method=\"post\" enctype=\"multipart/form-data\">\
                     <input type=\"hidden\" name=\"csrf_token\" value=\"{token}\">\
                     <input type=\"text\" name=\"title\" value=\"default-title\">\
                     <input type=\"file\" name=\"doc\">\
                     <input type=\"submit\" value=\"Upload\"></form>"
                ),
            )
        }
        ("POST", "/form/upload") => {
            let content_type = request.header("content-type").unwrap_or_default();
            let boundary = content_type
                .split(';')
                .find_map(|part| part.trim().strip_prefix("boundary="))
                .unwrap_or_default()
                .to_owned();
            let parts = parse_multipart(&request.body, &boundary);
            let fields = parts
                .iter()
                .filter(|part| part.filename.is_none())
                .map(|part| {
                    (
                        part.name.clone(),
                        String::from_utf8_lossy(&part.content).into_owned(),
                    )
                })
                .collect::<HashMap<_, _>>();
            if !state.take("upload", fields.get("csrf_token")) {
                return Response::html(403, "<html><body>bad csrf</body></html>");
            }
            let title = fields.get("title").cloned();
            let file = parts.iter().find(|part| part.filename.is_some());
            if let Some(file) = file {
                state.upload = Some((
                    file.filename.clone().unwrap_or_default(),
                    file.content.clone(),
                    title,
                ));
            }
            Response::html(200, "<html><body>upload-ok</body></html>")
        }
        ("GET", "/step1") => {
            let token = state.issue("step1");
            Response::html(
                200,
                format!(
                    "<form action=\"/step1\" method=\"post\">\
                     <input type=\"hidden\" name=\"csrf_token\" value=\"{token}\">\
                     <input type=\"text\" name=\"value\" value=\"one\">\
                     <input type=\"submit\" value=\"Next\"></form>"
                ),
            )
        }
        ("POST", "/step1") => {
            let fields = request.form();
            if !state.take("step1", fields.get("csrf_token")) {
                return Response::html(403, "<html><body>bad csrf step1</body></html>");
            }
            let token = state.issue("step2");
            Response::html(
                200,
                format!(
                    "<html><body>step1-done<form action=\"/step2\" method=\"post\">\
                     <input type=\"hidden\" name=\"csrf_token\" value=\"{token}\">\
                     <input type=\"text\" name=\"value\" value=\"two\">\
                     <input type=\"submit\" value=\"Finish\"></form></body></html>"
                ),
            )
        }
        ("POST", "/step2") => {
            let fields = request.form();
            if !state.take("step2", fields.get("csrf_token")) {
                return Response::html(403, "<html><body>bad csrf step2</body></html>");
            }
            Response::html(200, "<html><body>all-done</body></html>")
        }
        ("GET", "/list") => {
            let page: usize = request
                .query
                .get("page")
                .and_then(|value| value.parse().ok())
                .unwrap_or(1);
            let (items, has_next) = match page {
                1 => (10, true),
                2 => (10, true),
                _ => (5, false),
            };
            let mut body = String::from("<html><body><ul>");
            for index in 0..items {
                body.push_str(&format!("<li>item-{page}-{index}</li>"));
            }
            body.push_str("</ul>");
            if has_next {
                body.push_str(&format!(
                    "<a rel=\"next\" href=\"/list?page={}\">Next</a>",
                    page + 1
                ));
            }
            body.push_str("</body></html>");
            Response::html(200, body)
        }
        ("GET", "/loop") => {
            let page = request
                .query
                .get("page")
                .cloned()
                .unwrap_or_else(|| "a".to_owned());
            let other = if page == "a" { "b" } else { "a" };
            Response::html(
                200,
                format!(
                    "<html><body><ul><li>loop-{page}</li></ul>\
                     <a rel=\"next\" href=\"/loop?page={other}\">Next</a></body></html>"
                ),
            )
        }
        ("GET", "/form/multi") => {
            let token = state.issue("multi");
            let checked = request.query.get("checked").map(String::as_str);
            Response::html(
                200,
                multi_form(&token, checked == Some("a"), checked == Some("b")),
            )
        }
        ("POST", "/form/multi") => {
            let fields = request.form();
            if !state.take("multi", fields.get("csrf_token")) {
                return Response::html(403, "<html><body>bad csrf</body></html>");
            }
            state.record("/form/multi", fields);
            state.record_echo("/form/multi", &request.method, &request.body);
            Response::html(200, "<html><body>multi-ok</body></html>")
        }
        ("GET", "/form/redirect") => {
            let status = request
                .query
                .get("status")
                .cloned()
                .unwrap_or_else(|| "307".to_owned());
            let token = state.issue("redirect");
            Response::html(
                200,
                format!(
                    "<html><body><form action=\"/redirect/{status}\" method=\"post\">\
                     <input type=\"hidden\" name=\"csrf_token\" value=\"{token}\">\
                     <input type=\"text\" name=\"q\" value=\"default\">\
                     <input type=\"submit\" value=\"Go\"></form></body></html>"
                ),
            )
        }
        ("POST", "/redirect/307") => Response::redirect_with(307, "/echo/307"),
        ("POST", "/redirect/308") => Response::redirect_with(308, "/echo/308"),
        ("POST", "/redirect/303") => Response::redirect_with(303, "/echo/303"),
        ("GET", "/spa") => Response::html(
            200,
            "<html><body><div id=\"root\"></div>\
             <script src=\"/bundle.js\"></script>\
             <script>window.app = {};</script>\
             <script>window.boot = function() {};</script></body></html>",
        ),
        ("GET", "/plain") => Response::html(200, "<html><body>plain page</body></html>"),
        _ => Response::html(404, "<html><body>no route</body></html>"),
    }
}

fn login_page(token: &str, message: &str) -> String {
    format!(
        "<html><body><h1>Login</h1><p>{message}</p>\
         <form action=\"/login\" method=\"post\">\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{token}\">\
         <input type=\"text\" name=\"username\">\
         <input type=\"password\" name=\"password\">\
         <input type=\"submit\" name=\"commit\" value=\"Sign in\"></form></body></html>"
    )
}

fn basic_form(token: &str) -> String {
    format!(
        "<html><head><title>Basic form</title></head><body>\
         <form action=\"/form/landing\" method=\"post\">\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{token}\">\
         <input type=\"hidden\" name=\"lang\" value=\"zh-CN\">\
         <input type=\"text\" name=\"name\" value=\"default-name\">\
         <textarea name=\"note\">default-note</textarea>\
         <select name=\"color\">\
           <option value=\"red\">Red</option>\
           <option value=\"green\" selected>Green</option>\
           <option value=\"blue\">Blue</option>\
         </select>\
         <input type=\"checkbox\" name=\"agree\" value=\"yes\" checked>\
         <input type=\"radio\" name=\"plan\" value=\"free\">\
         <input type=\"radio\" name=\"plan\" value=\"pro\" checked>\
         <input type=\"submit\" name=\"go\" value=\"Send\">\
         </form></body></html>"
    )
}
fn multi_form(token: &str, checked_a: bool, checked_b: bool) -> String {
    format!(
        "<html><body><form action=\"/form/multi\" method=\"post\">\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{token}\">\
         <input type=\"checkbox\" name=\"tags\" value=\"a\"{a}>\
         <input type=\"checkbox\" name=\"tags\" value=\"b\"{b}>\
         <input type=\"text\" name=\"q\" value=\"one\">\
         <input type=\"text\" name=\"q\" value=\"two\">\
         <select name=\"colors\" multiple>\
           <option value=\"red\">Red</option>\
           <option value=\"green\" selected>Green</option>\
           <option value=\"blue\" selected>Blue</option>\
         </select>\
         <input type=\"submit\" value=\"Send\"></form></body></html>",
        a = if checked_a { " checked" } else { "" },
        b = if checked_b { " checked" } else { "" },
    )
}
