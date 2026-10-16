//! 礼貌层集成测试（离线：只连 127.0.0.1 上的本地测试服务器）。
//!
//! 用例编号对齐 docs/TESTING.md 的 2.10 POLITE-01…10。
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use webctl_core::ErrorCode;
use webctl_web::politeness::{PoliteFetcher, PolitenessConfig, RobotsOutcome, ThrottleConfig};
use webctl_web::transport::FetchOptions;

#[derive(Default)]
struct Record {
    started: Mutex<Vec<(String, Instant)>>,
    counts: Mutex<HashMap<String, usize>>,
    in_flight: AtomicUsize,
    peak_in_flight: AtomicUsize,
}

impl Record {
    fn count(&self, path: &str) -> usize {
        *self.counts.lock().unwrap().get(path).unwrap_or(&0)
    }
    fn timestamps(&self, path: &str) -> Vec<Instant> {
        self.started
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p == path)
            .map(|(_, t)| *t)
            .collect()
    }
}

struct Server {
    base: String,
    record: Arc<Record>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        let record = Arc::new(Record::default());
        let for_thread = Arc::clone(&record);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let record = Arc::clone(&for_thread);
                std::thread::spawn(move || {
                    let _ = handle(stream, record);
                });
            }
        });
        Self {
            base: format!("http://{addr}"),
            record,
        }
    }
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }
}

fn handle(mut stream: TcpStream, record: Arc<Record>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/")
        .to_owned();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
    }

    {
        let mut counts = record.counts.lock().unwrap();
        *counts.entry(path.clone()).or_insert(0) += 1;
    }
    let attempt = record.count(&path);
    record
        .started
        .lock()
        .unwrap()
        .push((path.clone(), Instant::now()));
    let now = record.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    record.peak_in_flight.fetch_max(now, Ordering::SeqCst);

    let response = route(&path, attempt);
    stream.write_all(&response)?;
    stream.flush()?;
    record.in_flight.fetch_sub(1, Ordering::SeqCst);
    Ok(())
}

fn route(path: &str, attempt: usize) -> Vec<u8> {
    match path {
        "/robots.txt" => ok("text/plain", b"User-agent: *\nDisallow: /private/\n"),
        "/private/x" => ok("text/plain", b"secret"),
        "/ok" => ok("text/plain", b"ok"),
        "/slow" => {
            std::thread::sleep(Duration::from_millis(150));
            ok("text/plain", b"slow")
        }
        "/429" if attempt == 1 => raw(
            429,
            "Too Many Requests",
            &[("Retry-After", "1"), ("Content-Type", "text/plain")],
            b"slow down",
        ),
        "/429" => ok("text/plain", b"after-retry"),
        "/challenge" => raw(
            403,
            "Forbidden",
            &[
                ("Server", "cloudflare"),
                ("Content-Type", "text/html"),
            ],
            b"<html><title>Just a moment...</title><div id=\"cf-chl\"></div></html>",
        ),
        "/boom" => raw(500, "Internal Server Error", &[("Content-Type", "text/plain")], b"boom"),
        _ => raw(404, "Not Found", &[("Content-Type", "text/plain")], b"nope"),
    }
}

fn ok(content_type: &str, body: &[u8]) -> Vec<u8> {
    raw(200, "OK", &[("Content-Type", content_type)], body)
}

fn raw(status: u16, reason: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut response = format!("HTTP/1.1 {status} {reason}\r\n");
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!("Content-Length: {}\r\n", body.len()));
    response.push_str("Connection: close\r\n\r\n");
    let mut bytes = response.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

fn unused_port_url() -> String {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    format!("http://{addr}/")
}

fn fetcher(server: &Server, mutate: impl FnOnce(&mut PolitenessConfig)) -> PoliteFetcher {
    let mut config = PolitenessConfig {
        retry_base_delay: Duration::from_millis(50),
        max_backoff: Duration::from_secs(2),
        ..PolitenessConfig::default()
    };
    mutate(&mut config);
    let options = FetchOptions {
        timeout: Duration::from_secs(5),
        ..FetchOptions::default()
    };
    let _ = server;
    PoliteFetcher::new(config, options)
}

/// POLITE-07：robots 禁止的路径**不会发起请求**。
#[tokio::test]
async fn robots_disallow_blocks_without_requesting() {
    let server = Server::start();
    let fetcher = fetcher(&server, |_| {});

    let envelope = fetcher.fetch(&server.url("/private/x")).await;
    assert!(!envelope.ok, "应当被 robots 拒绝");
    assert_eq!(envelope.code, ErrorCode::Blocked);
    assert!(envelope.error.contains("robots"), "错误：{}", envelope.error);
    assert_eq!(server.record.count("/private/x"), 0, "不得发起被禁止的请求");
}

/// POLITE-08：关闭 robots 时结果里记录"已关闭"这一事实。
#[tokio::test]
async fn robots_can_be_disabled_and_is_recorded() {
    let server = Server::start();
    let fetcher = fetcher(&server, |config| config.respect_robots = false);

    let envelope = fetcher.fetch(&server.url("/private/x")).await;
    assert!(envelope.ok, "{:?}", envelope.error);
    let data = envelope.data.expect("data");
    assert_eq!(data.robots, RobotsOutcome::Disabled);
    assert_eq!(server.record.count("/private/x"), 1);
}

/// POLITE-02：同主机请求间隔不小于配置值。
#[tokio::test]
async fn min_interval_is_enforced() {
    let server = Server::start();
    let fetcher = fetcher(&server, |config| {
        config.respect_robots = false;
        config.throttle = ThrottleConfig {
            max_concurrency_per_host: 4,
            min_interval: Duration::from_millis(200),
        };
    });

    fetcher.fetch(&server.url("/ok")).await;
    fetcher.fetch(&server.url("/ok")).await;

    let stamps = server.record.timestamps("/ok");
    assert_eq!(stamps.len(), 2);
    let gap = stamps[1].duration_since(stamps[0]);
    assert!(gap >= Duration::from_millis(180), "间隔只有 {gap:?}");
}

/// POLITE-01：每主机并发不超过上限。
#[tokio::test]
async fn concurrency_is_capped() {
    let server = Server::start();
    let fetcher = Arc::new(fetcher(&server, |config| {
        config.respect_robots = false;
        config.throttle = ThrottleConfig {
            max_concurrency_per_host: 2,
            min_interval: Duration::from_millis(0),
        };
    }));

    let mut tasks = Vec::new();
    for _ in 0..4 {
        let fetcher = Arc::clone(&fetcher);
        let url = server.url("/slow");
        tasks.push(tokio::spawn(async move { fetcher.fetch(&url).await }));
    }
    for task in tasks {
        let envelope = task.await.expect("task");
        assert!(envelope.ok, "{:?}", envelope.error);
    }
    let peak = server.record.peak_in_flight.load(Ordering::SeqCst);
    assert!(peak <= 2, "服务端观测到的峰值并发 {peak}");
}

/// POLITE-04：429 + Retry-After 被尊重，且重试有上限。
#[tokio::test]
async fn retry_after_is_respected() {
    let server = Server::start();
    let fetcher = fetcher(&server, |config| {
        config.respect_robots = false;
        config.max_retries = 2;
        config.retry_base_delay = Duration::from_millis(10);
    });

    let started = Instant::now();
    let envelope = fetcher.fetch(&server.url("/429")).await;
    let elapsed = started.elapsed();

    assert!(envelope.ok, "{:?}", envelope.error);
    let data = envelope.data.expect("data");
    assert_eq!(data.attempts, 2, "首次 429 + 一次重试");
    assert!(data.retried);
    assert!(
        elapsed >= Duration::from_millis(900),
        "Retry-After: 1 应当等约 1s，实际 {elapsed:?}"
    );
    assert_eq!(server.record.count("/429"), 2);
}

/// POLITE-09 / POLITE-10：拦截页归 BLOCKED，且不重试轰炸。
#[tokio::test]
async fn interception_is_blocked_without_retry_storm() {
    let server = Server::start();
    let fetcher = fetcher(&server, |config| {
        config.respect_robots = false;
        config.max_retries = 3;
    });

    let envelope = fetcher.fetch(&server.url("/challenge")).await;
    assert!(!envelope.ok);
    assert_eq!(envelope.code, ErrorCode::Blocked);
    assert!(
        envelope.error.contains("interception"),
        "错误：{}",
        envelope.error
    );
    assert_eq!(server.record.count("/challenge"), 1, "拦截不该重试");
}

/// POLITE-09 补充：普通 5xx 归 BLOCKED，不当作成功。
#[tokio::test]
async fn server_error_is_blocked() {
    let server = Server::start();
    let fetcher = fetcher(&server, |config| config.respect_robots = false);
    let envelope = fetcher.fetch(&server.url("/boom")).await;
    assert!(!envelope.ok);
    assert_eq!(envelope.code, ErrorCode::Blocked);
}

/// 退避重试对网络错误生效，但不会超过 max_retries。
#[tokio::test]
async fn network_errors_retry_within_budget() {
    let server = Server::start();
    let fetcher = fetcher(&server, |config| {
        config.respect_robots = false;
        config.max_retries = 1;
        config.retry_base_delay = Duration::from_millis(50);
        config.max_backoff = Duration::from_millis(100);
    });

    let started = Instant::now();
    let envelope = fetcher.fetch(&unused_port_url()).await;
    assert!(!envelope.ok);
    assert_eq!(envelope.code, ErrorCode::Network);
    assert!(
        started.elapsed() >= Duration::from_millis(40),
        "应当发生过一次退避等待"
    );
}
