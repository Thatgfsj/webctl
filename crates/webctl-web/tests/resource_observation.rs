//! 资源观测工具自测（议题 #22 §5）：RES-03/05/07/08/10、SEC-10、ENV-12、CAPS-06、MCP-14。
//!
//! Linux 上给出真实数字并断言；其他平台工具返回 None，断言显式跳过
//! （不把"测不了"伪装成"通过了"）。

#[path = "support/http_fixture.rs"]
mod http_fixture;
#[path = "support/resources.rs"]
mod resources;

use std::time::Duration;

use http_fixture::{Response, Server};
use webctl_core::{CapsReport, Config, ErrorCode};
use webctl_web::transport::{FetchOptions, fetch};

fn options() -> FetchOptions {
    FetchOptions::default()
}

/// 平台不支持时显式跳过，而不是静默通过。
fn expect_supported<T>(value: Option<T>) -> Option<T> {
    let supported = resources::PLATFORM_SUPPORTED;
    if value.is_none() {
        assert!(!supported, "Linux 上资源工具必须给出数字");
    }
    value
}

/// ENV-12 / RES-10：代表性 L0 负载（caps 自检 + 取回）前后子进程计数**不增加**。
///
/// TESTING 原文是"跑完整套 L0 用例后计数不增加"；单个测试无法观测套件级全局值，
/// 因此这里用 delta 形式覆盖本测试内的代表性负载：caps（core）+ fixture 取回（web）。
/// 全仓库静态零调用点由 offline_lane_has_no_subprocess_call_sites 与 SEC-10 另行保证。
#[tokio::test]
async fn env_12_no_child_processes() {
    let Some(before) = expect_supported(resources::child_process_count()) else {
        return;
    };
    for _ in 0..16 {
        let report = CapsReport::detect(&Config::default());
        assert_eq!(report.capabilities.len(), 5);
    }
    let server = Server::start_with(vec![("/tiny", Response::text("x"))]).await;
    for _ in 0..16 {
        let env = fetch(&server.url("/tiny"), &options()).await;
        assert!(env.ok, "{}", env.error);
    }
    let after = resources::child_process_count().expect("Linux 上第二次采样不应失败");
    assert!(
        after <= before,
        "L0 代表性负载不得新增子进程：before={before}, after={after}"
    );
}

/// SEC-10（静态检查）：源码中没有创建子进程的 API。
///
/// 扫描根来自 resources::crates_root()；根缺失时返回 Err，此处 expect 让它失败，
/// 而不是像修复前那样静默空集、恒真通过。
#[test]
fn sec_10_sources_have_no_subprocess_calls() {
    let sites = resources::subprocess_call_sites(&resources::crates_root())
        .expect("扫描源码必须成功（路径写错必须失败，而不是空集）");
    assert!(
        sites.is_empty(),
        "发现子进程调用点（{} 处）：{:#?}",
        sites.len(),
        sites
    );
}

/// 扫描器自检 1：仓库根定位正确，且扫描确实看到真实源码。
///
/// 直接防住历史根因：workspace_root() 曾返回 <repo>/crates，调用点再
/// .join("crates") 得到不存在的 <repo>/crates/crates。
#[test]
fn scanner_roots_point_at_real_sources() {
    let root = resources::crates_root();
    assert!(root.is_dir(), "crates 根必须存在：{}", root.display());
    let files = resources::rust_sources_under(&root).expect("扫描必须成功");
    assert!(
        files.len() > 10,
        "扫描必须看到真实源码，实际只有 {} 个文件（疑似路径算错）",
        files.len()
    );
    assert!(
        files
            .iter()
            .any(|file| file.ends_with("tests/support/resources.rs")),
        "扫描根必须覆盖测试支撑模块"
    );
    let manifest = resources::workspace_root().join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", manifest.display()));
    assert!(
        text.contains("[workspace]"),
        "workspace_root 必须指向含 [workspace] 的仓库根，实际 {}",
        resources::workspace_root().display()
    );
}

/// 扫描器自检 2：**故意植入违规**时必须检出——证明检查不是恒真。
///
/// 在临时目录写入含外站 URL 与 Command::new( 的 .rs，两个扫描器都必须报出来；
/// 再换成注释内容，验证注释剥离不会误报。
#[test]
fn scanners_detect_planted_violations() {
    let dir = std::env::temp_dir().join(format!("webctl-scan-selfcheck-{}", std::process::id()));
    let tests_dir = dir.join("tests");
    std::fs::create_dir_all(&tests_dir).expect("创建临时 tests 目录");
    let planted = tests_dir.join("planted.rs");
    // 片段拆开拼装：避免本测试自身被 offline lane 扫描命中。
    let bad_url = ["http", "://planted.external-host.net/x"].join("");
    let bad_call = ["Command", "::new(\"sh\")"].join("");
    let planted_source = format!("fn planted() {{ {bad_call}; let _ = \"{bad_url}\"; }}\n");
    std::fs::write(&planted, &planted_source).expect("写入植入样本");

    let urls = resources::external_url_sites(&dir).expect("URL 扫描必须成功");
    assert_eq!(urls.len(), 1, "植入的外站 URL 必须被检出：{urls:#?}");
    assert!(urls[0].path.ends_with("planted.rs"));
    assert_eq!(urls[0].line, 1);

    let calls = resources::subprocess_call_sites(&dir).expect("子进程扫描必须成功");
    assert_eq!(calls.len(), 1, "植入的子进程调用必须被检出：{calls:#?}");
    assert!(calls[0].path.ends_with("planted.rs"));

    let commented = format!("// {bad_call} 与 {bad_url}\nfn clean() {{}}\n");
    std::fs::write(&planted, &commented).expect("改写植入样本");
    assert!(
        resources::external_url_sites(&dir)
            .expect("URL 扫描必须成功")
            .is_empty(),
        "注释里的 URL 不应误报"
    );
    assert!(
        resources::subprocess_call_sites(&dir)
            .expect("子进程扫描必须成功")
            .is_empty(),
        "注释里的子进程调用不应误报"
    );

    std::fs::remove_dir_all(&dir).expect("清理临时目录");
}

/// 扫描器自检 3：路径写错（目录不存在）必须返回 Err，绝不静默空集。
#[test]
fn scanners_error_on_missing_root() {
    let missing = resources::crates_root().join("definitely-not-a-real-dir");
    let url_error = resources::external_url_sites(&missing).expect_err("缺失目录必须返回 Err");
    assert!(url_error.path().ends_with("definitely-not-a-real-dir"));
    let call_error = resources::subprocess_call_sites(&missing).expect_err("缺失目录必须返回 Err");
    assert!(call_error.path().ends_with("definitely-not-a-real-dir"));
    assert!(url_error.to_string().contains("扫描失败"));
}

/// fd 计数可用且为正；RSS 采样给出可断言数字。
#[test]
fn resource_observers_yield_numbers() {
    if let Some(fds) = expect_supported(resources::fd_count()) {
        assert!(fds > 0, "fd 计数应为正");
    }
    let mut sampler = resources::RssSampler::new();
    if let Some(first) = expect_supported(sampler.sample()) {
        assert!(first > 0, "RSS 应为正");
        let _ = sampler.sample();
        let peak = sampler.peak().expect("peak");
        let growth = sampler.growth().expect("growth");
        assert!(peak >= first, "峰值不应低于首次读数");
        assert_eq!(growth, sampler.last().expect("last") as i64 - first as i64);
    }
}

/// RES-08：空闲窗口内 CPU 占用接近 0（阈值放宽以避免 CI 抖动）。
#[test]
fn res_08_idle_cpu_is_near_zero() {
    // 用线程级采样：进程级会被 cargo 的并行测试线程抬高，不是忙等信号。
    if let Some(percent) = expect_supported(resources::thread_cpu_percent_over(
        Duration::from_millis(200),
    )) {
        assert!(
            percent < 25.0,
            "空闲线程 CPU 采样不应出现忙等（实测 {percent:.2}%）"
        );
    }
}

/// RES-07 / RES-10：错误路径不 panic、码可区分，且全程无子进程。
///
/// 覆盖 404/403/500/429（BLOCKED）、超时（TIMEOUT）、重定向环与超限（NETWORK）、
/// 连接失败（NETWORK）、5 类非法 URL（BAD_URL）、截断降级（LIMIT）。
#[tokio::test]
async fn res_07_error_paths_do_not_panic_and_stay_classified() {
    let server = Server::start_with(vec![
        ("/ok", Response::text("ok")),
        ("/missing", Response::status(404)),
        ("/forbidden", Response::status(403)),
        ("/boom", Response::status(500)),
        ("/rate-limited", Response::retry_after(1)),
        (
            "/slow",
            Response::text("late").delay(Duration::from_millis(300)),
        ),
        ("/loop-a", Response::redirect(302, "/loop-b")),
        ("/loop-b", Response::redirect(302, "/loop-a")),
        ("/large", Response::large(256 * 1024, b'x')),
    ])
    .await;

    let before = expect_supported(resources::child_process_count());

    let ok = fetch(&server.url("/ok"), &options()).await;
    assert!(ok.ok, "{}", ok.error);
    assert_eq!(ok.validate(), Ok(()));

    for (path, status) in [
        ("/missing", 404),
        ("/forbidden", 403),
        ("/boom", 500),
        ("/rate-limited", 429),
    ] {
        let env = fetch(&server.url(path), &options()).await;
        assert!(!env.ok, "{path} (HTTP {status})");
        assert_eq!(env.code, ErrorCode::Blocked, "{path} (HTTP {status})");
        assert_ne!(env.code, ErrorCode::Parse, "{path} (HTTP {status})");
        assert!(env.data.is_none(), "{path} (HTTP {status})");
        assert!(!env.error.is_empty(), "{path} (HTTP {status})");
        assert_eq!(env.validate(), Ok(()), "{path} (HTTP {status})");
    }

    let mut slow_options = options();
    slow_options.timeout = Duration::from_millis(30);
    let slow = fetch(&server.url("/slow"), &slow_options).await;
    assert_eq!(slow.code, ErrorCode::Timeout);
    assert_eq!(slow.validate(), Ok(()));

    let looped = fetch(&server.url("/loop-a"), &options()).await;
    assert_eq!(looped.code, ErrorCode::Network);
    assert_eq!(looped.validate(), Ok(()));

    let chain = Server::start_with(vec![
        ("/c0", Response::redirect(302, "/c1")),
        ("/c1", Response::redirect(302, "/c2")),
        ("/c2", Response::text("done")),
    ])
    .await;
    let mut bounded = options();
    bounded.max_redirects = 1;
    let limited = fetch(&chain.url("/c0"), &bounded).await;
    assert_eq!(limited.code, ErrorCode::Network);
    assert_eq!(limited.validate(), Ok(()));

    for bad in [
        "",
        "not a url",
        "/relative",
        "ftp://example.test/x",
        "file:///etc/passwd",
    ] {
        let env = fetch(bad, &options()).await;
        assert!(!env.ok, "{bad:?}");
        assert_eq!(env.code, ErrorCode::BadUrl, "{bad:?}");
        assert!(env.data.is_none(), "{bad:?}");
        assert!(!env.error.is_empty(), "{bad:?}");
        assert_eq!(env.validate(), Ok(()), "{bad:?}");
    }

    // 连接失败：先占端口再释放，随后请求该地址。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let refused = fetch(&format!("http://{addr}/"), &options()).await;
    assert_eq!(refused.code, ErrorCode::Network);
    assert_eq!(refused.validate(), Ok(()));

    // 截断是"降级"而非错误：码 LIMIT 且 degraded_from 非空。
    let mut truncating = options();
    truncating.max_body_bytes = 4096;
    let truncated = fetch(&server.url("/large"), &truncating).await;
    assert!(truncated.ok, "{}", truncated.error);
    assert_eq!(truncated.code, ErrorCode::Limit);
    assert!(truncated.meta.degraded_from.is_some());
    assert_eq!(truncated.validate(), Ok(()));

    if let (Some(before), Some(after)) = (before, resources::child_process_count()) {
        assert!(
            after <= before,
            "错误路径不得新增子进程：before={before}, after={after}"
        );
    }
}

/// RES-05：连续请求后 fd 数回到基线（允许很小的波动）。
#[tokio::test]
async fn res_05_fd_count_returns_to_baseline() {
    // 先 await 一次让 tokio 运行时把 epoll 等 fd 建好，再取基线。
    tokio::time::sleep(Duration::from_millis(20)).await;
    let baseline = expect_supported(resources::fd_count());
    let Some(baseline) = baseline else {
        return;
    };

    let server = Server::start_with(vec![("/tiny", Response::text("x"))]).await;
    for _ in 0..50 {
        let env = fetch(&server.url("/tiny"), &options()).await;
        assert!(env.ok, "{}", env.error);
    }
    drop(server);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let after = resources::fd_count().expect("after fd");
    assert!(
        after <= baseline + 4,
        "长跑后 fd 应回到基线：baseline={baseline}, after={after}"
    );
}

/// RES-03：1000 次连续请求后 RSS 增长有界（总增长 + 后段边际增长双断言）。
///
/// 阈值依据（本机实测：1 字节正文 + 独立连接，见 PR 评论）：
/// - 首 ~250 次请求存在一次性的运行时/分配器水位，约 6.2 MiB，之后进入平台期；
/// - 250 → 4000 次请求只再增长约 0.6 MiB（≈0.16 KiB/请求），不是线性泄漏；
/// 因此总阈值取 16 MiB（覆盖启动抖动，同时能抓住每请求 >= 16 KiB 的泄漏），
/// 后 500 次边际增长取 4 MiB（约为实测边际的一个量级上界，远紧于旧 64 MiB 口径）。
#[tokio::test]
async fn res_03_rss_growth_is_bounded_over_1000_requests() {
    let mut sampler = resources::RssSampler::new();
    let Some(first) = expect_supported(sampler.sample()) else {
        return;
    };
    let server = Server::start_with(vec![("/tiny", Response::text("x"))]).await;
    for _ in 0..500 {
        let env = fetch(&server.url("/tiny"), &options()).await;
        assert!(env.ok, "{}", env.error);
    }
    sampler.sample();
    for _ in 0..500 {
        let env = fetch(&server.url("/tiny"), &options()).await;
        assert!(env.ok, "{}", env.error);
    }
    sampler.sample();

    let first = first as i64;
    let middle = sampler.samples()[1] as i64;
    let last = sampler.samples()[2] as i64;
    let total = last - first;
    let marginal = last - middle;
    let total_mib = total as f64 / (1024.0 * 1024.0);
    let marginal_mib = marginal as f64 / (1024.0 * 1024.0);
    eprintln!("RES-03: 前 500 次增长 {total_mib:.2} MiB，后 500 次边际增长 {marginal_mib:.2} MiB");
    assert!(
        total < 16 * 1024 * 1024,
        "1000 次请求 RSS 总增长应有界（实测 {total} 字节 / {total_mib:.2} MiB，阈值 16 MiB）"
    );
    assert!(
        marginal < 4 * 1024 * 1024,
        "后 500 次边际增长应有界（实测 {marginal} 字节 / {marginal_mib:.2} MiB，阈值 4 MiB）"
    );
}
