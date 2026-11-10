//! #225（M5）：caps 声明 fallback 能力 + 降级信封对外部工具的提示。
//!
//! 本文件是 M5 的**唯一**测试位置（全部离线：不联网、不起子进程、不读写外部文件）：
//! - 解析：`<名字>:<工具名>` 合法 → Some；缺冒号 / 空字段 / 多冒号 / 含空白 → None；
//!   未设置 → None（用注入值 `resolve(None)` 表达，不改进程环境）；
//! - caps：有声明时出现 `fallback_capability`，无声明时该字段整体省略；工具数仍为 18；
//! - next_steps：未声明时不含任何外部工具名（断言无 `mcp__`）；
//!   声明存在且 `playwright_suitable=true` 时含 `use <tool>`。
//!
//! **环境前提**：本文件里走真实 `guard` 的用例假设运行环境**未设置**
//! `WEBCTL_FALLBACK_TOOL`（默认即如此）；用例开头会显式断言这一点，
//! 以免环境变量把"未声明"用例伪装成失败。

use webctl_core::fallback_decl::{self, FallbackDecl};
use webctl_core::{Config, Envelope, ErrorCode, FallbackHint, Trust};
use webctl_web::degrade::fallback;
use webctl_web::{DegradeOptions, Scope, guard};

const TOOL: &str = "mcp__playwright__browser_navigate";

/// 合成 JS 空壳：空的 #root + chunk 脚本 → 强信号，适合外部浏览器。
const JS_SHELL: &str = r#"<!DOCTYPE html>
<html><head><title>App</title>
<script src="/static/js/chunk-vendors.js"></script></head>
<body><div id="root"></div><script>window.__NUXT__ = {};</script></body></html>"#;

fn decl() -> FallbackDecl {
    FallbackDecl {
        name: "playwright".to_owned(),
        tool: TOOL.to_owned(),
    }
}

fn suitable_hint() -> FallbackHint {
    FallbackHint {
        playwright_suitable: true,
        signals: vec!["div#root empty".to_owned()],
        embedded_data_tried: false,
        js_engine_suitable: true,
    }
}

fn raw(html: &str) -> Envelope<String> {
    Envelope::ok(html.to_owned(), Trust::UntrustedPageContent, 1)
}

// ---------------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------------

/// 合法格式 → Some，且名字/工具名分段正确。
#[test]
fn parses_well_formed_declaration() {
    let parsed = fallback_decl::parse("playwright:mcp__playwright__browser_navigate")
        .expect("合法格式必须解析成功");
    assert_eq!(parsed.name, "playwright");
    assert_eq!(parsed.tool, "mcp__playwright__browser_navigate");
    assert_eq!(parsed.next_step(), format!("use {TOOL}"));

    // 允许两段首尾空白（格式仍是"名字:工具名"）。
    let trimmed = fallback_decl::parse(" playwright : mcp__playwright__browser_navigate ")
        .expect("首尾空白应被忽略");
    assert_eq!(trimmed, decl());
}

/// 非法格式一律 None：缺冒号、空字段、多冒号、含空白、空串。
#[test]
fn rejects_malformed_declaration() {
    for raw in [
        "",
        "playwright",
        ":mcp__playwright__browser_navigate",
        "playwright:",
        "playwright: ",
        "playwright:mcp__playwright__browser_navigate:extra",
        "play wright:mcp__playwright__browser_navigate",
        "playwright:mcp__playwright browser_navigate",
    ] {
        assert!(
            fallback_decl::parse(raw).is_none(),
            "非法格式必须返回 None：{raw:?}"
        );
    }
}

/// 变量未设置 → None（用注入值表达；edition 2024 下 `set_var` 是 unsafe，本 workspace 禁止）。
#[test]
fn unset_variable_yields_none() {
    assert!(fallback_decl::resolve(None).is_none());
    assert!(fallback_decl::resolve(Some("")).is_none());
    // 真实环境：本用例文件的前提是未声明（见文件头说明）。
    assert!(
        fallback_decl::from_env().is_none(),
        "本用例要求运行环境未设置 WEBCTL_FALLBACK_TOOL"
    );
}

// ---------------------------------------------------------------------------
// caps
// ---------------------------------------------------------------------------

/// 有声明时 caps 报告里出现 fallback_capability（{"name", "tool"}）。
#[test]
fn caps_includes_declared_fallback_capability() {
    let report = webctl_mcp::caps::report(&Config::default(), Vec::new(), Some(decl()));
    let value = serde_json::to_value(&report).expect("serialize");
    assert_eq!(
        value["fallback_capability"],
        serde_json::json!({"name": "playwright", "tool": TOOL}),
        "有声明时必须原样报出：{value}"
    );
}

/// 无声明时字段整体省略（向后兼容：老消费者看不到新字段）。
#[test]
fn caps_omits_field_without_declaration() {
    let report = webctl_mcp::caps::report(&Config::default(), Vec::new(), None);
    let value = serde_json::to_value(&report).expect("serialize");
    assert!(
        value.get("fallback_capability").is_none(),
        "未声明时不得出现 fallback_capability：{value}"
    );
}

/// 声明不新增工具：工具面仍是 18 个（caps 的 tools 与 TOOL_NAMES 同源同量）。
#[test]
fn declaration_does_not_add_tools() {
    assert_eq!(webctl_mcp::TOOL_NAMES.len(), 18, "工具数必须保持 18");
    let report = webctl_mcp::caps::report(
        &Config::default(),
        webctl_mcp::caps::tool_capabilities(),
        Some(decl()),
    );
    assert_eq!(report.tools.len(), webctl_mcp::TOOL_NAMES.len());
    assert!(
        report.tool("fallback_capability").is_none(),
        "fallback 声明不是工具"
    );
}

// ---------------------------------------------------------------------------
// next_steps
// ---------------------------------------------------------------------------

/// 未声明：降级信封的 next_steps 不含任何外部工具名（无 `mcp__`）。
///
/// 走真实 guard 路径（JS 空壳 + 默认判定），只读字符串，零网络。
#[test]
fn undeclared_next_steps_never_mention_external_tools() {
    assert!(
        fallback_decl::from_env().is_none(),
        "本用例要求运行环境未设置 WEBCTL_FALLBACK_TOOL"
    );
    let envelope = guard(
        JS_SHELL,
        raw(JS_SHELL),
        &DegradeOptions::default(),
        Scope::Fetch,
    );
    assert_eq!(envelope.code, ErrorCode::Degraded, "{}", envelope.error);
    let actionable = envelope.meta.actionable.expect("降级必须带 actionable");
    assert!(
        actionable
            .fallback_hint
            .as_ref()
            .expect("JS 空壳必须带 fallback_hint")
            .playwright_suitable
    );
    for step in &actionable.next_steps {
        assert!(
            !step.contains("mcp__"),
            "未声明时 next_steps 不得指向外部工具：{step:?}"
        );
    }
    // 接线点同样不应改动 next_steps。
    let mut steps = actionable.next_steps.clone();
    let before = steps.clone();
    fallback::push_declared_next_step(actionable.fallback_hint.as_ref(), &mut steps);
    assert_eq!(steps, before, "未声明时接线点必须什么都不做");
}

/// 声明存在 + playwright_suitable=true → next_steps 含 `use <tool>`（且不重复）。
#[test]
fn declared_and_suitable_appends_use_tool() {
    let hint = suitable_hint();
    let decl = decl();
    let mut steps = vec!["embedded_data".to_owned()];
    assert!(
        fallback_decl::push_next_step(&mut steps, &hint, Some(&decl)),
        "应当追加"
    );
    assert_eq!(
        steps,
        vec!["embedded_data".to_owned(), format!("use {TOOL}")]
    );

    // 幂等：同一条不重复追加。
    assert!(!fallback_decl::push_next_step(
        &mut steps,
        &hint,
        Some(&decl)
    ));
    assert_eq!(steps.len(), 2);

    assert_eq!(
        fallback_decl::next_step_for(&hint, Some(&decl)),
        Some(format!("use {TOOL}"))
    );
}

/// 声明存在但 playwright_suitable=false → 不追加（信号不足不得臆测）。
#[test]
fn declared_but_unsuitable_appends_nothing() {
    let hint = FallbackHint {
        playwright_suitable: false,
        ..suitable_hint()
    };
    let mut steps = vec!["embedded_data".to_owned()];
    assert!(!fallback_decl::push_next_step(
        &mut steps,
        &hint,
        Some(&decl())
    ));
    assert_eq!(steps, vec!["embedded_data".to_owned()]);
    assert!(fallback_decl::next_step_for(&hint, Some(&decl())).is_none());
}

/// 未声明（就算 playwright_suitable=true）→ 不追加。
#[test]
fn suitable_without_declaration_appends_nothing() {
    let hint = suitable_hint();
    let mut steps = Vec::new();
    assert!(!fallback_decl::push_next_step(&mut steps, &hint, None));
    assert!(steps.is_empty());
    assert!(fallback_decl::next_step_for(&hint, None).is_none());
}
