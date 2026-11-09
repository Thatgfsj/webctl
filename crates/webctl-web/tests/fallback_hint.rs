//! #224：fetch/extract 降级信封的结构化 fallback_hint（离线）。
//!
//! 三个合成 fixture 各自锁定一条验收：
//! - JS 空壳 → `playwright_suitable = true` 且 `signals` 非空；
//! - 纯静态页 → 不出现 `fallback_hint`（更不会降级）；
//! - 依赖外链库 → `js_engine_suitable = false`。
//!
//! 全部数据来自本地 fixture 字符串，零网络、零子进程。

use webctl_core::{Envelope, ErrorCode, Trust};
use webctl_web::degrade::fallback::hint;
use webctl_web::{DegradeOptions, Scope, guard};

const JS_SHELL: &str = include_str!("fixtures/fallback_js_shell.html");
const STATIC: &str = include_str!("fixtures/fallback_static.html");
const EXTERNAL_LIB: &str = include_str!("fixtures/fallback_external_lib.html");

fn raw(html: &str) -> Envelope<String> {
    Envelope::ok(html.to_owned(), Trust::UntrustedPageContent, 1)
}

/// JS 空壳：强信号存在 → 适合 Playwright，且信号逐条可核对。
#[test]
fn js_shell_hints_playwright() {
    let hint = hint(JS_SHELL, false).expect("JS 空壳必须产出 fallback_hint");
    assert!(hint.playwright_suitable);
    assert!(!hint.signals.is_empty(), "强信号命中时不得返回空 signals");
    assert!(
        hint.signals.iter().any(|signal| signal.contains("#root")),
        "应点名空 #root：{:?}",
        hint.signals
    );
    assert!(
        hint.signals.iter().any(|signal| signal.contains("chunk")),
        "应点名 chunk 脚本：{:?}",
        hint.signals
    );

    // 走真实的 fetch 守卫路径：DEGRADED + actionable.fallback_hint。
    let envelope = guard(
        JS_SHELL,
        raw(JS_SHELL),
        &DegradeOptions::default(),
        Scope::Fetch,
    );
    assert_eq!(envelope.code, ErrorCode::Degraded, "{}", envelope.error);
    let value = serde_json::to_value(&envelope).expect("serialize");
    let actionable = envelope.meta.actionable.expect("降级必须带 actionable");
    let hint = actionable.fallback_hint.expect("JS 页必须带 fallback_hint");
    assert!(hint.playwright_suitable);
    assert_eq!(
        value["meta"]["actionable"]["fallback_hint"]["playwright_suitable"],
        true
    );
}

/// 纯静态页：没有任何强信号 → hint 为 None，且守卫不降级。
#[test]
fn static_page_has_no_fallback_hint() {
    assert!(
        hint(STATIC, false).is_none(),
        "纯静态页不得出现 fallback_hint"
    );
    let envelope = guard(
        STATIC,
        raw(STATIC),
        &DegradeOptions::default(),
        Scope::Fetch,
    );
    assert_eq!(envelope.code, ErrorCode::Ok);
    assert!(envelope.meta.actionable.is_none());
    let value = serde_json::to_value(&envelope).expect("serialize");
    assert!(
        value["meta"].get("actionable").is_none(),
        "未降级时 meta 不得出现 actionable"
    );
}

/// 依赖外链库：内置 JS 引擎救不了 → js_engine_suitable = false。
#[test]
fn external_library_is_not_js_engine_suitable() {
    let hint = hint(EXTERNAL_LIB, false).expect("空壳必须产出 fallback_hint");
    assert!(hint.playwright_suitable);
    assert!(
        !hint.js_engine_suitable,
        "依赖外链库时内置引擎不应被标为可用"
    );

    let envelope = guard(
        EXTERNAL_LIB,
        raw(EXTERNAL_LIB),
        &DegradeOptions::default(),
        Scope::Fetch,
    );
    assert_eq!(envelope.code, ErrorCode::Degraded, "{}", envelope.error);
    let hint = envelope
        .meta
        .actionable
        .expect("actionable")
        .fallback_hint
        .expect("hint");
    assert!(!hint.js_engine_suitable);
}
