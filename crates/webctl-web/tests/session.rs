//! SESS-01…14 与 FORM-01…12 的离线集成测试（PLAN v0.4 §2「会话」域，PR 04）。
//!
//! 全部用例只访问 127.0.0.1 上的本地表单站，不访问外网、不启动子进程。
//! fixture 服务器写在本文件内（不修改 PR 02 的 tests/support/mod.rs）。

use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use webctl_core::ErrorCode;
use webctl_web::session::{
    FileUpload, FormFill, SessionEngine, SessionStore, validate_session_name,
};

// ---------------------------------------------------------------------------
// 本地表单站 fixture（手写最小 HTTP/1.1，支持 GET/POST/cookie/重定向/multipart）
// ---------------------------------------------------------------------------

mod site {
    use super::*;

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
            Self {
                status: 302,
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
            403 => "Forbidden",
            404 => "Not Found",
            _ => "Status",
        }
    }

    // ---- 路由处理 ----

    pub fn handle(request: &Request, state: &mut State) -> Response {
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
}

use site::{Site, parse_multipart};

// ---------------------------------------------------------------------------
// 测试辅助
// ---------------------------------------------------------------------------

fn temp_store(label: &str) -> SessionStore {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("webctl-pr04-{label}-{nanos}"));
    SessionStore::with_ttl(dir, Duration::from_secs(3600))
}

fn engine(label: &str) -> SessionEngine {
    SessionEngine::with_store("default", temp_store(label)).expect("engine")
}

fn credentials() -> FormFill {
    let mut fill = FormFill::new();
    fill.set("username", "alice").set("password", "secret");
    fill
}

// ---------------------------------------------------------------------------
// SESS
// ---------------------------------------------------------------------------

/// SESS-01：登录成功后受保护页面 200 且内容匹配。
#[tokio::test]
async fn sess_01_login_success() {
    let site = Site::start().await;
    let engine = engine("sess01");
    let env = engine
        .login(&site.url("/login"), None, &credentials())
        .await;
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.code, ErrorCode::Ok);
    assert_eq!(env.validate(), Ok(()));
    let page = env.data.expect("page");
    assert_eq!(page.status, 200);
    assert_eq!(page.url, site.url("/dashboard"));
    assert!(page.html.contains("Welcome alice"), "{}", page.html);
    assert!(engine.is_logged_in());

    let protected = engine.fetch_authenticated(&site.url("/dashboard")).await;
    assert!(protected.ok, "{}", protected.error);
    assert!(protected.data.expect("data").html.contains("Welcome alice"));
}

/// SESS-02：未登录访问受保护页面 → SESSION_LOST，且与其它失败码可区分。
#[tokio::test]
async fn sess_02_not_logged_in_is_session_lost() {
    let site = Site::start().await;
    let engine = engine("sess02");
    let env = engine.fetch_authenticated(&site.url("/dashboard")).await;
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::SessionLost);
    assert_ne!(env.code, ErrorCode::Blocked);
    assert_ne!(env.code, ErrorCode::Parse);
    assert_ne!(env.code, ErrorCode::Unsupported);
    assert!(!env.error.is_empty());
    assert!(env.data.is_none());
    assert_eq!(env.validate(), Ok(()));
}

/// SESS-03：cookie 持久化——新引擎从磁盘恢复后无需重新登录。
#[tokio::test]
async fn sess_03_cookie_persists_across_engines() {
    let site = Site::start().await;
    let store = temp_store("sess03");
    let first = SessionEngine::with_store("default", store.clone()).expect("first");
    assert!(
        first
            .login(&site.url("/login"), None, &credentials())
            .await
            .ok
    );
    first.save_state(&[]).expect("save state");

    let mut second = SessionEngine::restore("default", store).expect("second");
    let env = second.fetch_authenticated(&site.url("/dashboard")).await;
    assert!(env.ok, "{}", env.error);
    assert!(env.data.expect("data").html.contains("Welcome alice"));
    second.load_state().expect("load");
    assert!(second.cookie_jar().len() >= 1);
}

/// SESS-04：CSRF token 从页面提取并回填，服务端断言匹配。
#[tokio::test]
async fn sess_04_csrf_is_extracted_and_refilled() {
    let site = Site::start().await;
    let engine = engine("sess04");
    let login = engine
        .login(&site.url("/login"), None, &credentials())
        .await;
    assert!(login.ok, "{}", login.error);

    let page = engine.fetch_page(&site.url("/form/basic")).await;
    let page = page.data.expect("page");
    let expected = page.forms[0]
        .field("csrf_token")
        .expect("token")
        .value
        .clone();
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("csrf_token"), Some(&expected));
}

/// SESS-05：token 轮换后仍能提交（使用新页面的新 token）。
#[tokio::test]
async fn sess_05_csrf_rotation_is_refetched() {
    let site = Site::start().await;
    let engine = engine("sess05");
    let page1 = engine
        .fetch_page(&site.url("/step1"))
        .await
        .data
        .expect("page1");
    let token1 = page1.forms[0]
        .field("csrf_token")
        .expect("t1")
        .value
        .clone();
    let step1 = engine.submit_form(&page1, 0, &FormFill::new()).await;
    assert!(step1.ok, "{}", step1.error);
    let page2 = step1.data.expect("page2");
    let token2 = page2.forms[0]
        .field("csrf_token")
        .expect("t2")
        .value
        .clone();
    assert_ne!(token1, token2, "服务端必须轮换 token");
    assert!(page2.html.contains("step1-done"));

    let step2 = engine.submit_form(&page2, 0, &FormFill::new()).await;
    assert!(step2.ok, "轮换后重取失败：{}", step2.error);
    assert!(step2.data.expect("page3").html.contains("all-done"));
}

/// SESS-06：隐藏字段在提交时保留原值。
#[tokio::test]
async fn sess_06_hidden_fields_are_preserved() {
    let site = Site::start().await;
    let engine = engine("sess06");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("lang").map(String::as_str), Some("zh-CN"));
    assert_eq!(
        recorded.get("csrf_token").map(String::as_str),
        Some(
            page.forms[0]
                .field("csrf_token")
                .expect("token")
                .value
                .as_str()
        )
    );
}

/// SESS-07 + FORM-10：multipart 上传被服务端正确接收，字节数一致。
#[tokio::test]
async fn sess_07_form_10_multipart_upload() {
    let site = Site::start().await;
    let engine = engine("sess07");
    let page = engine
        .fetch_page(&site.url("/form/upload"))
        .await
        .data
        .expect("page");
    assert_eq!(
        page.forms[0].enctype,
        webctl_web::session::FormEnctype::Multipart
    );
    let mut fill = FormFill::new();
    fill.set("title", "报告").upload(
        "doc",
        FileUpload::new(
            "report.bin",
            "application/octet-stream",
            b"hello upload".to_vec(),
        ),
    );
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(env.ok, "{}", env.error);
    let (filename, bytes, title) = site
        .with_state(|state| state.upload.clone())
        .expect("upload");
    assert_eq!(filename, "report.bin");
    assert_eq!(bytes.as_slice(), b"hello upload");
    assert_eq!(title.as_deref(), Some("报告"));
}

/// SESS-08：分页遍历，各页条数之和 = 站点总数，且遵守页数上限。
#[tokio::test]
async fn sess_08_pagination_sums_items() {
    let site = Site::start().await;
    let engine = engine("sess08");
    let env = engine.paginate(&site.url("/list?page=1"), Some(10)).await;
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.code, ErrorCode::Ok);
    let paginated = env.data.expect("paginated");
    assert_eq!(paginated.pages.len(), 3);
    assert_eq!(paginated.item_counts, vec![10, 10, 5]);
    assert_eq!(paginated.total_items, 25);
    assert!(paginated.limit.is_none());
    assert!(paginated.next.is_none());

    // 页数上限：只取一页即停止并标 LIMIT。
    let limited = engine.paginate(&site.url("/list?page=1"), Some(1)).await;
    assert_eq!(limited.code, ErrorCode::Limit);
    assert_eq!(
        limited.meta.degraded_from.as_deref(),
        Some("pagination-max-pages")
    );
    assert_eq!(limited.data.expect("data").pages.len(), 1);
}

/// SESS-09：循环 next（A→B→A）在上限内停止并标记 LIMIT。
#[tokio::test]
async fn sess_09_loop_pagination_stops_with_limit() {
    let site = Site::start().await;
    let engine = engine("sess09");
    let env = engine.paginate(&site.url("/loop?page=a"), Some(10)).await;
    assert_eq!(env.code, ErrorCode::Limit);
    assert_eq!(env.meta.degraded_from.as_deref(), Some("pagination-loop"));
    assert_eq!(env.validate(), Ok(()));
    let paginated = env.data.expect("data");
    assert!(paginated.pages.len() <= 10, "必须在上限内停止");
    assert_eq!(paginated.pages.len(), 2);
    assert_eq!(paginated.limit.as_deref(), Some("pagination-loop"));
}

/// SESS-10：SPA 页面返回 DEGRADED 且 degraded_from 非空，不伪造成功。
#[tokio::test]
async fn sess_10_spa_degrades() {
    let site = Site::start().await;
    let engine = engine("sess10");
    let env = engine.fetch_page(&site.url("/spa")).await;
    assert_eq!(env.code, ErrorCode::Degraded);
    assert_eq!(env.meta.degraded_from.as_deref(), Some("js-render"));
    assert_eq!(env.validate(), Ok(()));
    let page = env.data.expect("page");
    assert!(page.spa.is_some());
    assert!(page.spa.as_ref().expect("signal").reason.contains("JS"));
    assert!(page.html.contains("id=\"root\""));
}

/// SESS-11：登录态导出 → 新会话导入 → 受保护页可访问。
#[tokio::test]
async fn sess_11_export_import_session_state() {
    let site = Site::start().await;
    let store = temp_store("sess11");
    let first = SessionEngine::with_store("default", store.clone()).expect("first");
    assert!(
        first
            .login(&site.url("/login"), None, &credentials())
            .await
            .ok
    );
    let state = first.export_state(&[site.url("/").trim_end_matches('/').to_owned()]);
    let json = state.export_json().expect("export");
    let imported = webctl_web::session::SessionState::import_json(&json).expect("import");
    assert_eq!(imported.cookies.len(), state.cookies.len());

    let mut second = SessionEngine::with_store("default", store).expect("second");
    second.import_state(&imported);
    let env = second.fetch_authenticated(&site.url("/dashboard")).await;
    assert!(env.ok, "{}", env.error);
    assert!(env.data.expect("data").html.contains("Welcome alice"));
}

/// SESS-12：非法会话名（路径穿越等）被拒，不落到 state 目录外。
#[tokio::test]
async fn sess_12_invalid_session_name_is_rejected() {
    let store = temp_store("sess12");
    let root = store.sessions_dir();
    for bad in ["../escape", "a/b", "a\\b", "", ".."] {
        assert!(validate_session_name(bad).is_err(), "{bad:?}");
        assert!(store.path_for(bad).is_err(), "{bad:?}");
        let err = SessionEngine::with_store(bad, store.clone()).expect_err("reject");
        assert_eq!(err.code(), ErrorCode::Blocked);
    }
    let outside = root.parent().expect("parent").join("escape.json");
    assert!(!outside.exists(), "不得在 state 目录外创建文件");
    assert!(!root.join("..").join("escape.json").exists());
}

/// SESS-13：登录后 302 回跳原页面，会话保持。
#[tokio::test]
async fn sess_13_redirect_login_keeps_session() {
    let site = Site::start().await;
    let engine = engine("sess13");
    // 未登录访问 /dashboard 会被 302 到 /login。
    let before = engine.fetch_page(&site.url("/dashboard")).await;
    assert!(before.ok, "{}", before.error);
    let page = before.data.expect("page");
    assert_eq!(page.url, site.url("/login"), "应跟随 302 到登录页");

    let env = engine
        .login(&site.url("/login"), None, &credentials())
        .await;
    assert!(env.ok, "{}", env.error);
    let landed = env.data.expect("landed");
    assert_eq!(landed.url, site.url("/dashboard"), "登录后应 302 回跳");
    let after = engine.fetch_authenticated(&site.url("/dashboard")).await;
    assert!(after.ok, "{}", after.error);
}

/// SESS-14：同一会话并发使用时结果正确（无串号）。
#[tokio::test]
async fn sess_14_concurrent_requests_share_session() {
    let site = Site::start().await;
    let engine = engine("sess14");
    assert!(
        engine
            .login(&site.url("/login"), None, &credentials())
            .await
            .ok
    );
    let dashboard = site.url("/dashboard");
    let (a, b) = tokio::join!(engine.fetch_page(&dashboard), engine.fetch_page(&dashboard));
    for env in [a, b] {
        assert!(env.ok, "{}", env.error);
        assert!(env.data.expect("data").html.contains("Welcome alice"));
    }
    assert!(site.hits("/dashboard") >= 2);
}

// ---------------------------------------------------------------------------
// FORM
// ---------------------------------------------------------------------------

/// FORM-01：单字段填值。
#[tokio::test]
async fn form_01_single_field() {
    let site = Site::start().await;
    let engine = engine("form01");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let mut fill = FormFill::new();
    fill.set("name", "Alice");
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("name").map(String::as_str), Some("Alice"));
}

/// FORM-02：一次填多个字段。
#[tokio::test]
async fn form_02_multiple_fields_at_once() {
    let site = Site::start().await;
    let engine = engine("form02");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let mut fill = FormFill::new();
    fill.set("name", "Bob").set("note", "hello note");
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("name").map(String::as_str), Some("Bob"));
    assert_eq!(recorded.get("note").map(String::as_str), Some("hello note"));
}

/// FORM-03：中文值按 UTF-8 正确编码传输。
#[tokio::test]
async fn form_03_unicode_value() {
    let site = Site::start().await;
    let engine = engine("form03");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let mut fill = FormFill::new();
    fill.set("name", "中文名字");
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("name").map(String::as_str), Some("中文名字"));
}

/// FORM-04：select 选中项与提交值一致。
#[tokio::test]
async fn form_04_select_value() {
    let site = Site::start().await;
    let engine = engine("form04");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(
        recorded.get("color").map(String::as_str),
        Some("green"),
        "默认选中 green"
    );

    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let mut fill = FormFill::new();
    fill.set("color", "blue");
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("color").map(String::as_str), Some("blue"));
}

/// FORM-05：checkbox / radio 勾选与不勾选都符合 HTML 语义。
#[tokio::test]
async fn form_05_checkbox_and_radio_semantics() {
    let site = Site::start().await;
    let engine = engine("form05");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("agree").map(String::as_str), Some("yes"));
    assert_eq!(recorded.get("plan").map(String::as_str), Some("pro"));

    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let mut fill = FormFill::new();
    fill.check("agree", false).select_radio("plan", "free");
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert!(!recorded.contains_key("agree"), "未勾选应不提交");
    assert_eq!(recorded.get("plan").map(String::as_str), Some("free"));
}

/// FORM-06：隐藏字段未被覆盖时保留原值。
#[tokio::test]
async fn form_06_hidden_field_default() {
    let site = Site::start().await;
    let engine = engine("form06");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("lang").map(String::as_str), Some("zh-CN"));
}

/// FORM-07：提交不存在的字段 → 明确错误（不静默忽略、不发起请求）。
#[tokio::test]
async fn form_07_unknown_field_is_error() {
    let site = Site::start().await;
    let engine = engine("form07");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let before = site.hits("/form/landing");
    let mut fill = FormFill::new();
    fill.set("nope", "1");
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Parse);
    assert!(env.error.contains("nope"), "{}", env.error);
    assert_eq!(site.hits("/form/landing"), before, "不得发起请求");
    assert_eq!(env.validate(), Ok(()));
}

/// FORM-08：缺必填字段 → 可读错误（不发起请求）。
#[tokio::test]
async fn form_08_missing_required() {
    let site = Site::start().await;
    let engine = engine("form08");
    let page = engine
        .fetch_page(&site.url("/form/required"))
        .await
        .data
        .expect("page");
    let before = site.hits("/form/required");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::Parse);
    assert!(env.error.contains("must"), "{}", env.error);
    assert_eq!(site.hits("/form/required"), before);
}

/// FORM-11：提交后返回最终 URL 与状态可追溯。
#[tokio::test]
async fn form_11_landing_url_and_status() {
    let site = Site::start().await;
    let engine = engine("form11");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let landed = env.data.expect("landed");
    assert_eq!(landed.url, site.url("/form/landing"));
    assert_eq!(landed.status, 200);
    assert!(landed.html.contains("landed"));
}

/// FORM-12：同一 token 重复提交被显式拒绝（客户端幂等保护）。
#[tokio::test]
async fn form_12_duplicate_submission_refused() {
    let site = Site::start().await;
    let engine = engine("form12");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let first = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(first.ok, "{}", first.error);
    let hits_after_first = site.hits("/form/landing");
    let second = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(!second.ok);
    assert_eq!(second.code, ErrorCode::Blocked);
    assert!(second.error.contains("duplicate"), "{}", second.error);
    assert_eq!(
        site.hits("/form/landing"),
        hits_after_first,
        "第二次不得真正发送"
    );
}

/// 守护断言：fixture 的 multipart 解析能识别文件与文本字段（自检）。
#[test]
fn fixture_multipart_parser_sanity() {
    let body = b"--B\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nhi\r\n--B\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nbytes\r\n--B--\r\n";
    let parts = parse_multipart(body, "B");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].name, "title");
    assert_eq!(parts[1].filename.as_deref(), Some("a.txt"));
    assert_eq!(parts[1].content.as_slice(), b"bytes");
}
