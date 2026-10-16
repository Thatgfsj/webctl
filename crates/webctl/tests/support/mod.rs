//! CLI 集成测试夹具：本地 HTTP 服务与临时配置文件。
//!
//! 只绑定 127.0.0.1，不访问外网、不依赖宿主机环境。服务线程随测试进程退出而结束。
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 固定注入的响应 cookie 值：任何输出都不允许出现它（CLI-10）。
pub const SECRET_COOKIE_VALUE: &str = "SUPER_SECRET_TOKEN";

/// 本地 fixture HTTP 服务；同时可充当 HTTP 代理（只看请求计数）。
pub struct Fixture {
    addr: SocketAddr,
    hits: Arc<AtomicUsize>,
}

impl Fixture {
    /// 启动服务：任何请求都返回 200 与固定正文，并带一个机密 cookie。
    pub fn start(body: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let addr = listener.local_addr().expect("fixture addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        std::thread::spawn(move || {
            for incoming in listener.incoming() {
                let Ok(mut stream) = incoming else { break };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                read_request(&mut stream);
                counter.fetch_add(1, Ordering::SeqCst);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\nset-cookie: session={SECRET_COOKIE_VALUE}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        Self { addr, hits }
    }

    /// http://127.0.0.1:<port>
    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// 拼接绝对 URL。
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    /// 收到的请求数。
    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

fn read_request(stream: &mut TcpStream) {
    let mut chunk = [0u8; 1024];
    let mut buffer = Vec::new();
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
}

/// 临时 TOML 配置文件，drop 时删除。
pub struct TempFile {
    path: PathBuf,
}

impl TempFile {
    /// 写入内容并返回句柄。
    pub fn new(tag: &str, contents: &str) -> Self {
        let path = unique_temp_path(tag, "toml");
        std::fs::write(&path, contents).expect("write temp config");
        Self { path }
    }

    /// 文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// 一个肯定不存在的临时路径（用作缺省配置文件或显式缺失用例）。
pub fn missing_path(tag: &str) -> PathBuf {
    unique_temp_path(tag, "missing")
}

/// 路径转字符串（跨平台）。
pub fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn unique_temp_path(tag: &str, extension: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "webctl-cli-{tag}-{}-{nanos}.{extension}",
        std::process::id()
    ))
}
