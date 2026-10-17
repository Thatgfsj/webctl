//! 离线 lane（议题 #22 §3）：ENV-12 / CAPS-06 / HTTP-20 / DOC-12 / MCP-14 / SEC-10 的基建前置。
//!
//! 两个通道都跑同一批 fixture：
//! - 默认门禁：cargo test
//! - L0 离线通道：cargo test --features offline（额外执行更严格的"零外网/零子进程"扫描）
//!
//! 本文件自身不得出现真实外站 URL（扫描见下），否则会扫描到自己。
//! 扫描规则见 tests/support/resources.rs 与 tests/golden/README.md。

#[path = "support/http_fixture.rs"]
mod http_fixture;
#[path = "support/resources.rs"]
mod resources;

use http_fixture::{Response, Server};
use webctl_core::{CapsReport, Config};

/// 平台不支持时显式跳过，而不是静默通过。
fn child_count_or_skip() -> Option<usize> {
    let value = resources::child_process_count();
    if value.is_none() {
        let supported = resources::PLATFORM_SUPPORTED;
        assert!(!supported, "Linux 上子进程计数必须给出数字");
    }
    value
}

/// 默认门禁与 L0 共用同一 fixture，且 fixture 只绑回环地址。
#[tokio::test]
async fn fixture_is_loopback_only_in_both_gates() {
    let server = Server::start_with(vec![("/", Response::text("ok"))]).await;
    assert!(server.addr().ip().is_loopback());
    assert!(server.base().starts_with("http://127.0.0.1:"));
}

/// CAPS-06：caps 能力自检（core 的 CapsReport::detect）运行期不创建子进程。
///
/// 这是**真实调用点**：直接驱动 caps 实现，而不是只快照测试进程当前的子进程数。
#[test]
fn caps_06_caps_detect_spawns_no_children() {
    let Some(before) = child_count_or_skip() else {
        return;
    };
    for _ in 0..64 {
        let report = CapsReport::detect(&Config::default());
        assert_eq!(report.capabilities.len(), 5, "caps 报告应含 5 个能力域");
    }
    let after = resources::child_process_count().expect("Linux 上第二次采样不应失败");
    assert_eq!(after, before, "caps 自检不得创建子进程");
}

/// L0 通道的严格断言 1：crates 下测试源码不出现任何真实外站 URL。
///
/// 扫描器在目录缺失时返回 Err（此处 expect）而不是空集，
/// 保证"路径写错"会变成测试失败而非假绿。
#[cfg(feature = "offline")]
#[test]
fn offline_lane_has_no_external_urls_in_tests() {
    let sites = resources::external_url_sites(&resources::crates_root())
        .expect("扫描测试源码必须成功（路径写错必须失败，而不是空集）");
    assert!(
        sites.is_empty(),
        "离线 lane 禁止外站 URL（命中 {} 处）：\n{:#?}",
        sites.len(),
        sites
    );
}

/// L0 通道的严格断言 2：crates 下源码不出现会创建子进程的 API（SEC-10 静态检查）。
#[cfg(feature = "offline")]
#[test]
fn offline_lane_has_no_subprocess_call_sites() {
    let sites = resources::subprocess_call_sites(&resources::crates_root())
        .expect("扫描源码必须成功（路径写错必须失败，而不是空集）");
    assert!(
        sites.is_empty(),
        "发现子进程调用点（{} 处）：{:#?}",
        sites.len(),
        sites
    );
}

/// DOC-12：文档处理全程零子进程（**真实调用点**）。
///
/// PR 07（#36）已合并，交接条件成立：本用例真的调用两个公开入口——
/// `doc_text`（HTML → 正文）与 `json_query`（JSON → 查询），并在调用前后各采一次
/// `child_process_count`，断言增量为 0。不再是 pending 占位。
///
/// 分工提醒：`doc_text` 对 JSON 是 `UNSUPPORTED`（`degraded_from == "use-json-query"`），
/// 故 JSON 走 `json_query`，两条路径都覆盖。
#[test]
fn doc_12_document_processing_spawns_no_children() {
    use webctl_web::{DocOptions, JsonQueryOptions, doc_text, json_query};

    let Some(before) = child_count_or_skip() else {
        return;
    };

    // 1) HTML 文档 → 正文（doc/mod.rs：HTML 交给解析层，失败透传其 code）。
    let html = b"<html><body><article><h1>T</h1><p>hello doc</p></article></body></html>";
    let doc = doc_text(html, &DocOptions::default());
    assert!(doc.ok, "HTML 正文提取应成功：{:?}", doc.error);

    // 2) JSON → 查询（内置 JSON 查询子集，替代外部 jq）。
    let json = br#"{"items":[{"id":1},{"id":2}]}"#;
    let queried = json_query(json, "$.items[*].id", &JsonQueryOptions::default());
    assert!(queried.ok, "json_query 应成功：{:?}", queried.error);

    let after = resources::child_process_count().expect("Linux 上第二次采样不应失败");
    assert_eq!(after, before, "文档处理不得创建子进程");
}

// MCP-14 的真实用例在 `crates/webctl-mcp/tests/mcp14.rs`
// （`mcp_14_in_process_dispatch_spawns_no_children`）：那里单独成一个测试二进制，
// 子进程计数基线不会被其它用例干扰。此处原先的 pending 占位是**重复**，已删除。
