//! 离线 lane（议题 #22 §3）：ENV-12 / CAPS-06 / HTTP-20 / DOC-12 / MCP-14 / SEC-10 的基建前置。
//!
//! 两个通道都跑同一批 fixture：
//! - 默认门禁：cargo test
//! - L0 离线通道：cargo test --features offline（额外执行更严格的"零外网/零子进程"扫描）
//!
//! 本文件自身不得出现真实外站 URL（扫描见下），否则会扫描到自己。

#[path = "support/http_fixture.rs"]
mod http_fixture;
#[path = "support/resources.rs"]
mod resources;

use http_fixture::{Response, Server};

/// 默认门禁与 L0 共用同一 fixture，且 fixture 只绑回环地址。
#[tokio::test]
async fn fixture_is_loopback_only_in_both_gates() {
    let server = Server::start_with(vec![("/", Response::text("ok"))]).await;
    assert!(server.addr().ip().is_loopback());
    assert!(server.base().starts_with("http://127.0.0.1:"));
}

/// L0 通道的严格断言 1：crates 下测试源码不出现任何真实外站 URL。
#[cfg(feature = "offline")]
#[test]
fn offline_lane_has_no_external_urls_in_tests() {
    let scheme_http = ["http", "://"].join("");
    let scheme_https = ["https", "://"].join("");
    let mut offenders = Vec::new();
    let root = resources::workspace_root().join("crates");
    for file in resources::rust_sources_under(&root) {
        let path = file.to_string_lossy().replace('\\', "/");
        if !path.contains("/tests/") || path.contains("/tests/support/") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (line_number, line) in text.lines().enumerate() {
            for (index, _) in line.match_indices(&scheme_http) {
                let rest = &line[index + scheme_http.len()..];
                let host_end = rest
                    .find(['/', '"', '\'', ')', ' ', ':', ','])
                    .unwrap_or(rest.len());
                let host = &rest[..host_end];
                let is_https = line[index..].starts_with(&scheme_https);
                let loopback = host == "127.0.0.1"
                    || host == "localhost"
                    || host == "[::1]"
                    || host == "::1"
                    || host.starts_with("127.");
                if is_https || !loopback {
                    offenders.push(format!("{}:{}: {}", path, line_number + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "离线 lane 禁止外站 URL：\n{}",
        offenders.join("\n")
    );
}

/// L0 通道的严格断言 2：crates 下源码不出现会创建子进程的 API（SEC-10 静态检查）。
#[cfg(feature = "offline")]
#[test]
fn offline_lane_has_no_subprocess_call_sites() {
    let sites = resources::subprocess_call_sites(&resources::workspace_root().join("crates"));
    assert!(sites.is_empty(), "发现子进程调用点：{sites:?}");
}
