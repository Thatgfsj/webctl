//! PR 12 安全负面用例（SEC-04 / 06 / 07 / 08 / 11 / 14 / 15）。
//!
//! 前置：**默认配置**（不加任何放宽开关）；L3。
//! 离线：只访问 `127.0.0.1` 上进程内的原始 TCP fixture，不访问外网、不启动子进程。
//! 断言一律用正则/结构化比较，避免“看起来像”的恒真检查。
//!
//! SEC-09（MCP 信任标记）归 PR 11；SEC-10 / SEC-12 已由 PR 13 / PR 05 覆盖，这里复用不重写。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use regex::Regex;
use webctl_core::ErrorCode;
use webctl_core::security::paths::{
    ArgumentError, SafeRoot, ensure_http_scheme, resolve_file_argument,
};
use webctl_core::security::redact::redact_log_line;
use webctl_web::paths::{
    DEFAULT_MAX_DOWNLOAD_BYTES, DownloadError, DownloadPolicy, commit_download,
};
use webctl_web::transport::{FetchOptions, fetch};

/// 注入的机密值：任何输出/日志出现它即判失败。
const SECRET: &str = "SUPER_SECRET_TOKEN";

fn matches(pattern: &str, text: &str) -> bool {
    Regex::new(pattern)
        .expect("测试正则必须可编译")
        .is_match(text)
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "webctl-web-sec-{tag}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// 进程内的原始 HTTP/1.1 fixture：记录每次请求头，响应由闭包决定。
struct RawServer {
    addr: SocketAddr,
    received: Arc<Mutex<String>>,
    _handle: std::thread::JoinHandle<()>,
}

impl RawServer {
    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    fn received(&self) -> String {
        self.received.lock().expect("capture lock").clone()
    }
}

fn spawn_http<F>(responder: F) -> RawServer
where
    F: Fn(&str) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let received = Arc::new(Mutex::new(String::new()));
    let captured = Arc::clone(&received);
    let _handle = std::thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(mut stream) = incoming else { break };
            let head = read_head(&mut stream);
            captured.lock().expect("capture lock").push_str(&head);
            let response = responder(&head);
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    RawServer {
        addr,
        received,
        _handle,
    }
}

fn read_head(stream: &mut TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
                if buffer.len() > 64 * 1024 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

fn ok_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

/// SEC-04：信封中 cookie 值与 URL query token 都被脱敏（正则断言）。
#[tokio::test]
async fn sec_04_envelope_redacts_cookie_and_query_token() {
    let server = spawn_http(|_| {
        format!(
            "HTTP/1.1 200 OK\r\nset-cookie: sid={SECRET}; Path=/; HttpOnly\r\ncontent-type: text/html\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok"
        )
    });
    let envelope = fetch(
        &server.url(&format!("/page?token={SECRET}&user=bob")),
        &FetchOptions::default(),
    )
    .await;
    assert!(envelope.ok, "{}", envelope.error);
    let serialized = serde_json::to_string(&envelope).expect("serialize");
    assert!(
        !serialized.contains(SECRET),
        "信封不得含 cookie / token 明文：{serialized}"
    );
    assert!(matches(r"sid=<redacted>", &serialized), "{serialized}");
    assert!(matches(r"token=<redacted>", &serialized), "{serialized}");
    assert!(
        matches(r"user=bob", &serialized),
        "非敏感 query 必须保留：{serialized}"
    );
}

/// SEC-06：跨域重定向不泄漏 `Authorization`（自造跨域 fixture）。
#[tokio::test]
async fn sec_06_cross_origin_redirect_drops_authorization() {
    let target = spawn_http(|_| ok_response("target"));
    let target_port = target.addr.port();
    let entry = spawn_http(move |_| {
        format!(
            "HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:{target_port}/landing\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        )
    });
    let options = FetchOptions {
        headers: vec![("Authorization".to_owned(), format!("Bearer {SECRET}"))],
        ..FetchOptions::default()
    };
    let envelope = fetch(&entry.url("/start"), &options).await;
    assert!(envelope.ok, "{}", envelope.error);

    let target_head = target.received();
    assert!(
        !matches(r"(?i)authorization", &target_head),
        "跨源目标不得收到 Authorization：{target_head}"
    );
    assert!(
        !target_head.contains(SECRET),
        "跨源目标不得收到凭据：{target_head}"
    );
    assert!(
        matches(r"(?i)authorization", &entry.received()),
        "同源首跳必须携带 Authorization（对照组，证明不是恒真）"
    );
}

/// SEC-06 对照组：同源重定向仍携带 `Authorization`。
#[tokio::test]
async fn sec_06_same_origin_redirect_keeps_authorization() {
    let server = spawn_http(|head| {
        if head.starts_with("GET /start") {
            "HTTP/1.1 302 Found\r\nlocation: /next\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned()
        } else {
            ok_response("next")
        }
    });
    let options = FetchOptions {
        headers: vec![("Authorization".to_owned(), format!("Bearer {SECRET}"))],
        ..FetchOptions::default()
    };
    let envelope = fetch(&server.url("/start"), &options).await;
    assert!(envelope.ok, "{}", envelope.error);
    assert!(
        matches(r"(?i)authorization", &server.received()),
        "同源重定向不得误删 Authorization"
    );
}

/// SEC-07：URL query 里的 token 在日志中脱敏（正则断言，且不得重复占位）。
#[test]
fn sec_07_url_query_tokens_are_redacted_in_logs() {
    let logged = redact_log_line(&format!(
        "GET http://127.0.0.1:1/x?token={SECRET}&user=bob HTTP/1.1"
    ));
    assert!(!logged.contains(SECRET), "{logged}");
    assert!(matches(r"token=<redacted>", &logged), "{logged}");
    assert!(matches(r"user=bob", &logged), "{logged}");
    assert!(
        !logged.contains(">\n") && !logged.contains("<redacted><redacted>"),
        "不得出现重复占位：{logged}"
    );
}

/// SEC-08：表单密码字段值不落日志。
#[test]
fn sec_08_password_form_fields_are_redacted_in_logs() {
    let logged = redact_log_line(&format!(
        "POST body user=bob&password={SECRET}&csrf={SECRET}"
    ));
    assert!(!logged.contains(SECRET), "{logged}");
    assert!(matches(r"password=<redacted>", &logged), "{logged}");
    assert!(matches(r"csrf=<redacted>", &logged), "{logged}");
}

/// SEC-14：下载类型白名单与大小上限生效。
#[test]
fn sec_14_download_whitelist_and_size_limit() {
    let policy = DownloadPolicy::default();
    assert!(
        policy
            .check(Some("text/html; charset=utf-8"), Some(10), 10)
            .is_ok()
    );
    for disallowed in [
        "application/zip",
        "application/octet-stream",
        "text/event-stream",
    ] {
        assert!(
            matches!(
                policy.check(Some(disallowed), None, 1),
                Err(DownloadError::TypeNotAllowed(_))
            ),
            "{disallowed} 必须被拒绝"
        );
    }
    assert!(matches!(
        policy.check(None, None, 1),
        Err(DownloadError::MissingContentType)
    ));
    let over = u64::try_from(DEFAULT_MAX_DOWNLOAD_BYTES).expect("usize→u64") + 1;
    let error = policy
        .check(Some("text/plain"), Some(over), 0)
        .expect_err("超限必须拒绝");
    assert!(matches!(error, DownloadError::TooLarge { .. }), "{error:?}");
    assert_eq!(error.code(), ErrorCode::Limit);
}

/// SEC-14（落盘侧）：通过门禁才写文件，且写入 0600、不越界。
#[test]
fn sec_14_commit_download_enforces_policy_and_containment() {
    let root_dir = temp_dir("commit");
    let root = SafeRoot::new(root_dir.clone());
    let policy = DownloadPolicy::default();

    assert!(matches!(
        commit_download(&root, "a.zip", Some("application/zip"), None, b"x", &policy),
        Err(DownloadError::TypeNotAllowed(_))
    ));
    assert!(matches!(
        commit_download(
            &root,
            "big.txt",
            Some("text/plain"),
            Some(u64::MAX),
            b"x",
            &policy
        ),
        Err(DownloadError::TooLarge { .. })
    ));
    let path = commit_download(
        &root,
        "page.html",
        Some("text/html"),
        Some(4),
        b"<h1>",
        &policy,
    )
    .expect("允许的下载必须成功");
    assert!(path.starts_with(&root_dir));
    assert_eq!(std::fs::read(&path).expect("read"), b"<h1>");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777,
            0o600
        );
    }
    std::fs::remove_dir_all(&root_dir).ok();
}

/// SEC-11：文件参数经 canonicalize，`../`、绝对路径、越界 symlink 全部拒绝。
#[test]
fn sec_11_file_arguments_are_canonicalized() {
    let root = temp_dir("paths");
    assert!(matches!(
        resolve_file_argument(&root, Path::new("../escape.json")),
        Err(ArgumentError::Unsafe(_))
    ));
    assert!(matches!(
        resolve_file_argument(&root, Path::new("/etc/passwd")),
        Err(ArgumentError::Absolute { .. })
    ));

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = temp_dir("paths-outside");
        let victim = outside.join("victim.html");
        std::fs::write(&victim, b"keep").expect("victim");
        symlink(&victim, root.join("link.html")).expect("symlink");
        let safe_root = SafeRoot::new(root.clone());
        assert!(matches!(
            commit_download(
                &safe_root,
                "link.html",
                Some("text/html"),
                None,
                b"overwrite",
                &DownloadPolicy::default()
            ),
            Err(DownloadError::Unsafe(_))
        ));
        assert_eq!(std::fs::read(&victim).expect("victim"), b"keep");
        std::fs::remove_dir_all(&outside).ok();
    }
    std::fs::remove_dir_all(&root).ok();
}

/// SEC-15：`file://` 等非 http(s) 在发起连接前被拒，且错误不泄漏本地路径。
#[tokio::test]
async fn sec_15_dangerous_schemes_are_rejected() {
    for bad in [
        "file:///etc/passwd",
        "data:text/plain,hi",
        "ftp://127.0.0.1/x",
    ] {
        let envelope = fetch(bad, &FetchOptions::default()).await;
        assert!(!envelope.ok, "{bad} 必须失败");
        assert_eq!(envelope.code, ErrorCode::BadUrl, "{bad}");
        assert!(
            !envelope.error.contains("/etc/passwd"),
            "错误信息不得泄漏本地路径：{}",
            envelope.error
        );
    }
    assert!(ensure_http_scheme("file:///etc/passwd").is_err());
    assert!(ensure_http_scheme("https://127.0.0.1/x").is_ok());
}
