//! PR 09 降级语义的离线集成测试（docs/TESTING.md §2.11 DEGRADE-01…10、§2.5 SESS-10、
//! §2.17 E2E-01/02 的 L0 等价物）。
//!
//! 全部用例只读本地 fixture、内存字符串与 127.0.0.1 上的进程内 fixture 服务：
//! 零外网、零子进程。真站锚点（DEGRADE-01/02、E2E-01/02）放在
//! tests/e2e/run-real.sh，可跳过、不作 CI 门禁；本文件用本地样本做等价断言。
//!
//! 边界（DEGRADE-06 与 DEGRADE-08 必须同时成立）：
//! - 静态页（哪怕脚本很多）零命中 -> code=OK（PR 06 的 EXTRACT-06 不变）；
//! - 判定为需要 JS 的页面零命中 -> code != OK（DEGRADED 或 UNSUPPORTED）。

#[path = "support/http_fixture.rs"]
mod http_fixture;

use http_fixture::{Response, Server};
use webctl_core::{Envelope, ErrorCode, Trust};
use webctl_web::extract::{ExtractOptions, select_matches};
use webctl_web::transport::FetchOptions;
use webctl_web::{
    Assessment, DEGRADED_FROM, DegradeOptions, Scope, Severity, assess, fetch, guard,
};

const STATIC_QUOTES: &str = include_str!("fixtures/degrade/static_quotes_10.html");
const JS_SHELL: &str = include_str!("fixtures/degrade/js_data_script_shell.html");
const SPA_SHELL: &str = include_str!("fixtures/degrade/spa_shell.html");
const NOSCRIPT_ONLY: &str = include_str!("fixtures/degrade/noscript_only.html");
const EMPTY_MAIN: &str = include_str!("fixtures/degrade/empty_main_with_scripts.html");
const STATIC_SCRIPTS: &str = include_str!("fixtures/degrade/static_scripts_with_text.html");

fn options() -> ExtractOptions {
    ExtractOptions::default()
}

fn raw(html: &str) -> Envelope<String> {
    Envelope::ok(html.to_owned(), Trust::UntrustedPageContent, 1)
}

fn assert_valid<T: std::fmt::Debug>(envelope: &Envelope<T>) {
    assert_eq!(
        envelope.validate(),
        Ok(()),
        "envelope invariants: {envelope:?}"
    );
}

/// DEGRADE-01 的 L0 等价物：像真站 /js 页那样，引语只存在于内联脚本里。
#[test]
fn degrade_01_l0_js_data_shell_is_degraded() {
    let envelope = guard(
        JS_SHELL,
        raw(JS_SHELL),
        &DegradeOptions::default(),
        Scope::Fetch,
    );
    assert_valid(&envelope);
    assert_eq!(envelope.code, ErrorCode::Degraded);
    assert_eq!(envelope.meta.degraded_from.as_deref(), Some(DEGRADED_FROM));
    assert!(!envelope.error.is_empty());
    assert_eq!(
        envelope.data.as_deref(),
        Some(JS_SHELL),
        "原始 HTML 原样返回"
    );
}

/// DEGRADE-02 的 L0 等价物：静态对照页稳定取到 10 条，且不因脚本多而降级。
#[test]
fn degrade_02_l0_static_anchor_yields_ten_quotes() {
    assert_eq!(assess(STATIC_QUOTES), Assessment::Rendered);
    let envelope = select_matches(STATIC_QUOTES, "div.quote", &options());
    assert_valid(&envelope);
    assert_eq!(envelope.code, ErrorCode::Ok);
    assert!(envelope.meta.degraded_from.is_none());
    let result = envelope.data.expect("extract data");
    assert_eq!(result.matches.len(), 10);
}

/// DEGRADE-03：诊断必须点名脚本密度、空容器、noscript、框架指纹四类依据。
#[test]
fn degrade_03_diagnosis_names_all_four_signal_kinds() {
    let Assessment::NeedsJs { diagnosis, .. } = assess(EMPTY_MAIN) else {
        panic!("empty main + scripts 应判为需要 JS");
    };
    for needle in [
        "empty container",
        "script density",
        "noscript",
        "framework fingerprint",
    ] {
        assert!(
            diagnosis.reason.contains(needle),
            "诊断缺少依据 {needle}: {}",
            diagnosis.reason
        );
    }
    let envelope = guard(
        EMPTY_MAIN,
        raw(EMPTY_MAIN),
        &DegradeOptions::default(),
        Scope::Fetch,
    );
    assert_valid(&envelope);
    assert_eq!(envelope.code, ErrorCode::Degraded);
    assert_eq!(envelope.meta.degraded_from.as_deref(), Some(DEGRADED_FROM));
    assert!(envelope.error.contains("script density"));
}

/// DEGRADE-04：仅 noscript 里有内容 -> DEGRADED。
#[test]
fn degrade_04_noscript_only_page_is_degraded() {
    let envelope = guard(
        NOSCRIPT_ONLY,
        raw(NOSCRIPT_ONLY),
        &DegradeOptions::default(),
        Scope::Fetch,
    );
    assert_valid(&envelope);
    assert_eq!(envelope.code, ErrorCode::Degraded);
    assert!(envelope.error.contains("noscript"), "{}", envelope.error);
    assert_eq!(envelope.meta.degraded_from.as_deref(), Some(DEGRADED_FROM));
}

/// DEGRADE-05：主容器为空且脚本多 -> DEGRADED。
#[test]
fn degrade_05_empty_main_container_with_scripts_is_degraded() {
    let envelope = guard(
        EMPTY_MAIN,
        raw(EMPTY_MAIN),
        &DegradeOptions::default(),
        Scope::Fetch,
    );
    assert_valid(&envelope);
    assert_eq!(envelope.code, ErrorCode::Degraded);
    assert!(
        envelope.error.contains("empty container"),
        "{}",
        envelope.error
    );
}

/// DEGRADE-06：静态但脚本多的页面不得判为降级；零命中仍是 EXTRACT-06 的 OK。
#[test]
fn degrade_06_static_page_with_many_scripts_is_not_degraded() {
    assert_eq!(assess(STATIC_SCRIPTS), Assessment::Rendered);
    let envelope = guard(
        STATIC_SCRIPTS,
        raw(STATIC_SCRIPTS),
        &DegradeOptions::default(),
        Scope::Fetch,
    );
    assert_valid(&envelope);
    assert_eq!(envelope.code, ErrorCode::Ok);
    assert!(envelope.meta.degraded_from.is_none());

    // 边界：静态页零命中必须是 OK（PR 06 的 EXTRACT-06 原样保留）。
    let zero = select_matches(STATIC_SCRIPTS, "div.never-matches", &options());
    assert_valid(&zero);
    assert_eq!(zero.code, ErrorCode::Ok);
    assert!(zero.meta.degraded_from.is_none());
    assert!(zero.data.expect("data").matches.is_empty());
}

/// DEGRADE-07：强制开关跳过判定，原样返回原始 HTML，且 degraded_from 非空。
#[test]
fn degrade_07_force_switch_returns_raw_html_with_degraded_from() {
    // 不加强制开关时，同一静态页不降级。
    let plain = guard(
        STATIC_SCRIPTS,
        raw(STATIC_SCRIPTS),
        &DegradeOptions::default(),
        Scope::Fetch,
    );
    assert_eq!(plain.code, ErrorCode::Ok);

    let forced = guard(
        STATIC_SCRIPTS,
        raw(STATIC_SCRIPTS),
        &DegradeOptions { force: true },
        Scope::Fetch,
    );
    assert_valid(&forced);
    assert_eq!(forced.code, ErrorCode::Degraded);
    assert_eq!(forced.meta.degraded_from.as_deref(), Some(DEGRADED_FROM));
    assert!(!forced.error.is_empty());
    assert_eq!(
        forced.data.as_deref(),
        Some(STATIC_SCRIPTS),
        "强制开关必须返回原始 HTML"
    );
}

/// DEGRADE-08：需要 JS 的页面零命中 -> code 必须非 OK，且不得编造命中。
#[test]
fn degrade_08_zero_hits_on_js_page_is_never_ok() {
    let envelope = select_matches(JS_SHELL, "div.quote", &options());
    assert_valid(&envelope);
    assert!(envelope.ok);
    assert_ne!(envelope.code, ErrorCode::Ok);
    assert_eq!(envelope.meta.degraded_from.as_deref(), Some(DEGRADED_FROM));
    let result = envelope.data.expect("data");
    assert!(result.matches.is_empty(), "不得编造命中");
    assert!(!result.conclusion.is_empty());
}

/// DEGRADE-09：SPA 空壳（连占位都没有）-> UNSUPPORTED，data 说明为何无内容。
#[test]
fn degrade_09_spa_shell_zero_hits_is_unsupported_with_explanation() {
    assert!(matches!(
        assess(SPA_SHELL),
        Assessment::NeedsJs {
            severity: Severity::Total,
            ..
        }
    ));
    let envelope = select_matches(SPA_SHELL, "div.quote", &options());
    assert_valid(&envelope);
    assert_eq!(envelope.code, ErrorCode::Unsupported);
    assert_eq!(envelope.meta.degraded_from.as_deref(), Some(DEGRADED_FROM));
    let result = envelope.data.expect("data");
    assert!(result.matches.is_empty());
    assert!(
        result.conclusion.contains("0 elements"),
        "data 必须说明为何无内容：{}",
        result.conclusion
    );

    // 同一个空壳走 fetch 入口时原始 HTML 仍在 data 里 -> DEGRADED（同样非 OK）。
    let fetched = guard(
        SPA_SHELL,
        raw(SPA_SHELL),
        &DegradeOptions::default(),
        Scope::Fetch,
    );
    assert_valid(&fetched);
    assert_eq!(fetched.code, ErrorCode::Degraded);
}

/// DEGRADE-10：守卫绝不补全/改写内容；data 逐字节不变，也不编造命中。
#[test]
fn degrade_10_guard_never_rewrites_or_fabricates_content() {
    for html in [
        STATIC_QUOTES,
        JS_SHELL,
        SPA_SHELL,
        NOSCRIPT_ONLY,
        EMPTY_MAIN,
        STATIC_SCRIPTS,
    ] {
        let before = raw(html);
        let before_value = serde_json::to_value(&before).expect("serialize before");
        let after = guard(html, before, &DegradeOptions::default(), Scope::Fetch);
        let after_value = serde_json::to_value(&after).expect("serialize after");
        assert_eq!(
            after_value["data"], before_value["data"],
            "guard 不得修改 data"
        );
    }

    // 需要 JS 的空壳上零命中，绝不编造命中或补全正文。
    let envelope = select_matches(SPA_SHELL, "div.quote", &options());
    let result = envelope.data.expect("data");
    assert!(result.matches.is_empty());
    assert_eq!(result.observed_matches, 0);
}

/// 取回层接入点（PR 09 的唯一 fetch 调用点）的端到端离线验证：
/// 本地 fixture 服务返回 JS 空壳时 fetch 必须标 DEGRADED；静态页仍是 OK。
#[tokio::test]
async fn degrade_fetch_entry_marks_js_shell_degraded_offline() {
    let server = Server::start_with(vec![
        ("/js", Response::html(JS_SHELL)),
        ("/static", Response::html(STATIC_QUOTES)),
    ])
    .await;
    let options = FetchOptions::default();

    let js = fetch(&server.url("/js"), &options).await;
    assert_valid(&js);
    assert_eq!(js.meta.trust, Trust::UntrustedPageContent);
    assert_eq!(js.code, ErrorCode::Degraded);
    assert_eq!(js.meta.degraded_from.as_deref(), Some(DEGRADED_FROM));
    let body = js.data.expect("fetched").content.text.expect("html text");
    assert_eq!(body, JS_SHELL, "响应体必须原样保留");

    let static_page = fetch(&server.url("/static"), &options).await;
    assert_valid(&static_page);
    assert_eq!(static_page.code, ErrorCode::Ok);
    assert!(static_page.meta.degraded_from.is_none());
}
