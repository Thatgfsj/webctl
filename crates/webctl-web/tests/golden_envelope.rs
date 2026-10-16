//! ENV-07：信封 schema 快照 + golden 更新流程（议题 #22 §4）。
//!
//! 快照冻结字段名与类型；新增字段必须显式更新基线，避免"悄悄改协议"。
//! 更新流程见 tests/golden/README.md。

#[path = "support/golden.rs"]
mod golden;

use webctl_core::{Envelope, ErrorCode, Trust};

/// ENV-07：ok / fail / degraded 三种信封的字段名与类型快照。
#[test]
fn env_07_envelope_schema_matches_golden() {
    let ok = Envelope::ok(
        serde_json::json!({"kind": "html", "text": "hi"}),
        Trust::UntrustedPageContent,
        12,
    );
    let fail: Envelope<serde_json::Value> =
        Envelope::fail(ErrorCode::Blocked, "HTTP 404 Not Found at fixture", 5);
    let degraded = Envelope::degraded(
        serde_json::json!({"truncated": true}),
        "response-body-truncated",
        ErrorCode::Limit,
        7,
    );

    let shape = serde_json::json!({
        "ok": serde_json::to_value(&ok).expect("serialize ok"),
        "fail": serde_json::to_value(&fail).expect("serialize fail"),
        "degraded": serde_json::to_value(&degraded).expect("serialize degraded"),
    });
    golden::assert_json_golden("envelope_schema", &shape);
}

/// golden 更新流程必须有文档（机械检查字面量，避免流程失传）。
#[test]
fn golden_update_flow_is_documented() {
    let readme = golden::golden_root().join("README.md");
    let text = std::fs::read_to_string(&readme)
        .unwrap_or_else(|error| panic!("golden 说明缺失 {}: {error}", readme.display()));
    assert!(text.contains("UPDATE_GOLDEN"), "README 必须写明更新命令");
    assert!(text.contains("git diff"), "README 必须要求审阅 diff");
}
