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

/// DOC-12（pending）：文档处理全程零子进程。
///
/// **依赖**：文档与数据层（PDF 文本层 / json_query / 格式转换）归属 PR 07，
/// 当前 webctl-web 尚未提供任何文档处理入口，无法构造真实调用点。
/// **可验收交接点**：PR 07 合并后，调用其公开入口处理一份样本 PDF/JSON，
/// 在调用前后各采一次 child_process_count，断言增量为 0，并移除本 #[ignore]。
/// 现在显式标为 pending，不用恒真断言冒充覆盖。
#[test]
#[ignore = "pending PR 07：文档处理层未落地，无可断言的零子进程调用点"]
fn doc_12_document_processing_spawns_no_children() {
    panic!("DOC-12 尚未实现：PR 07 落地文档层后，用真实调用点替换本占位");
}

/// MCP-14（pending）：MCP 运行期无子进程、无浏览器。
///
/// **依赖**：MCP server（rmcp + 15 个工具）归属 PR 11；当前 webctl-mcp 只有 crate 骨架。
/// **可验收交接点**：PR 11 合并后启动 MCP server 进程，跑一次 initialize + tools/list，
/// 断言该进程期间的子进程计数不变；进程内 MCP 层则直接按零子进程断言。
/// 现在显式标为 pending，不用恒真断言冒充覆盖。
#[test]
#[ignore = "pending PR 11：MCP server 未落地（webctl-mcp 仅骨架），无可断言的运行期"]
fn mcp_14_server_runs_without_child_processes() {
    panic!("MCP-14 尚未实现：PR 11 落地 MCP server 后用真实运行期替换本占位");
}
