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
use webctl_core::ErrorCode;
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

/// ENV-12 / CAPS-06 / MCP-14：运行期不创建子进程。
#[test]
fn env_12_no_child_processes() {
    if let Some(children) = expect_supported(resources::child_process_count()) {
        assert_eq!(children, 0, "测试进程不得有子进程");
    }
}

/// SEC-10（静态检查）：源码中没有创建子进程的 API。
#[test]
fn sec_10_sources_have_no_subprocess_calls() {
    let sites = resources::subprocess_call_sites(&resources::workspace_root().join("crates"));
    assert!(sites.is_empty(), "发现子进程调用点：{sites:?}");
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

/// RES-10 / RES-07：混合负载后仍无子进程；错误路径不 panic 且给出可区分错误码。
#[tokio::test]
async fn res_10_no_subprocess_and_error_paths_do_not_panic() {
    let server = Server::start_with(vec![
        ("/ok", Response::text("ok")),
        ("/missing", Response::status(404)),
        (
            "/slow",
            Response::text("late").delay(Duration::from_millis(300)),
        ),
        ("/loop-a", Response::redirect(302, "/loop-b")),
        ("/loop-b", Response::redirect(302, "/loop-a")),
    ])
    .await;

    let ok = fetch(&server.url("/ok"), &options()).await;
    assert!(ok.ok);

    let missing = fetch(&server.url("/missing"), &options()).await;
    assert_eq!(missing.code, ErrorCode::Blocked);

    let mut slow_opts = options();
    slow_opts.timeout = Duration::from_millis(30);
    let slow = fetch(&server.url("/slow"), &slow_opts).await;
    assert_eq!(slow.code, ErrorCode::Timeout);

    let looped = fetch(&server.url("/loop-a"), &options()).await;
    assert_eq!(looped.code, ErrorCode::Network);

    let bad = fetch("not a url", &options()).await;
    assert_eq!(bad.code, ErrorCode::BadUrl);

    if let Some(children) = expect_supported(resources::child_process_count()) {
        assert_eq!(children, 0);
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

/// RES-03：1000 次连续请求后 RSS 增长有界。
#[tokio::test]
async fn res_03_rss_growth_is_bounded_over_1000_requests() {
    let mut sampler = resources::RssSampler::new();
    let Some(first) = expect_supported(sampler.sample()) else {
        return;
    };
    let server = Server::start_with(vec![("/tiny", Response::text("x"))]).await;
    for _ in 0..1000 {
        let env = fetch(&server.url("/tiny"), &options()).await;
        assert!(env.ok, "{}", env.error);
    }
    sampler.sample();
    let growth = sampler.last().expect("last") as i64 - first as i64;
    assert!(
        growth < 64 * 1024 * 1024,
        "1000 次请求 RSS 增长应有界（实测 {} MiB）",
        growth / (1024 * 1024)
    );
}
