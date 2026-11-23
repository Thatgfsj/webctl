//! `extract` 的 `fields` 参数：按 schema 抽字段（议题 #193）。
//!
//! 设计边界（与 `select_matches` 一致，且**不改变**原有输出）：
//! - 一个请求里声明多组 `name -> {selector, attr, all}`，只解析一次文档；
//! - `attr = null` 取元素文本（折叠空白）；否则取该属性原值（如 `src`/`href`）；
//! - `all = true` 返回数组（按文档序），否则只取第一个命中；
//! - 字段缺失 / 属性缺失 / 零命中都如实给 \`null\` 或空数组，并写入 `diagnostics`，
//!   **不**把单个字段的失败升级成整个请求失败；
//! - 只有「全部字段都是非法参数」或「fields 为空对象」才是整体 PARSE；
//!   全部字段都因超长/超复杂被守卫拒绝时是整体 BLOCKED（沿用选择器守卫语义）；
//! - `all=true` 的数组被 `max_results` 截断时是整体 **LIMIT** 降级
//!   （`degraded_from = "fields-truncated"`，同时 `diagnostics` 里有 `truncated`
//!   条目）。截断是降级不是失败：data 仍完整可用。
//!
//! 重名说明：`fields` 在 JSON 里是对象，重复键在 JSON 解析阶段就已被后者覆盖
//! （serde_json 保留最后一个），本层拿不到重复信息；字段名本身仍会被严格校验。
//!
//! 稳定性：字段值同样只是**单次解析**的坐标，跨请求不保证命中同一元素（见
//! `STABILITY_NOTE`）。

use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use webctl_core::{Envelope, ErrorCode, Trust};

use crate::parse::elapsed_ms;
use crate::parse::html::{collapse_ws, compile_selector, parse_document};

use super::{ExtractOptions, STABILITY_NOTE, budget_timeout, guard_selector};

/// `fields` 允许声明的最大字段数（防止一次调用扫描无界）。
pub const MAX_FIELDS: usize = 32;
/// 字段名最大 Unicode 字符数。
pub const MAX_FIELD_NAME_CHARS: usize = 64;

/// 单个字段的抽取声明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldSpec {
    /// CSS 选择器（沿用既有选择器引擎）。
    pub selector: String,
    /// `None` 取文本；`Some(attr)` 取该属性值。
    #[serde(default)]
    pub attr: Option<String>,
    /// `true` 返回数组（按文档序）；`false` 只取第一个命中。
    #[serde(default)]
    pub all: bool,
}

/// 一个字段的值。
///
/// 序列化后就是普通 JSON：`all=false` 时是字符串或 `null`；`all=true` 时是数组
/// （数组中可含 `null`，表示该命中元素缺少所取属性）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldValue {
    /// `all=false` 命中且取到值。
    Text(String),
    /// `all=true` 的数组（可含 `null`）；零命中时为空数组。
    List(Vec<Option<String>>),
    /// 字段缺失（零命中或取属性失败）。
    Null,
}

/// 字段级诊断类型。序列化用 kebab-case，便于机器分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FieldIssueKind {
    /// 字段名不合法。
    InvalidName,
    /// 选择器为空或无法编译。
    InvalidSelector,
    /// 选择器超长 / 超复杂，被既有守卫拒绝。
    BlockedSelector,
    /// 合法选择器但零命中（合法空结果）。
    ZeroMatches,
    /// 命中了元素，但该元素没有声明的属性。
    MissingAttribute,
    /// `attr` 参数本身非法（空串 / 含空白或控制字符）。
    InvalidAttribute,
    /// `all=true` 的数组被 `max_results` 截断。
    Truncated,
}

/// 一条字段级诊断。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDiagnostic {
    /// 字段名（原样回显，含非法名）。
    pub field: String,
    /// 诊断类型。
    pub kind: FieldIssueKind,
    /// 可读说明（含选择器、属性名等证据）。
    pub message: String,
}

/// `fields` 模式的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldSetResult {
    /// 字段名 -> 值；声明过的字段一定出现（缺失为 `null`）。
    pub fields: BTreeMap<String, FieldValue>,
    /// 字段级诊断；全部成功时为空数组。
    pub diagnostics: Vec<FieldDiagnostic>,
    /// 可读结论。
    pub conclusion: String,
    /// 稳定性声明，恒为 `STABILITY_NOTE`。
    pub stability: String,
}

impl FieldSetResult {
    /// 非 HTML 内容的说明性结果（UNSUPPORTED 降级用）。
    pub(crate) fn unsupported(reason: String) -> Self {
        Self {
            fields: BTreeMap::new(),
            diagnostics: Vec::new(),
            conclusion: reason,
            stability: STABILITY_NOTE.to_owned(),
        }
    }
}

/// 按 schema 抽字段。语义见模块文档。
pub fn extract_fields(
    html: &str,
    specs: &BTreeMap<String, FieldSpec>,
    options: &ExtractOptions,
) -> Envelope<FieldSetResult> {
    let started = Instant::now();
    let empty = validate_declaration(specs);
    if let Some((code, message)) = empty {
        return Envelope::fail(code, message, elapsed_ms(started));
    }
    let document = parse_document(html);
    if let Some(envelope) = budget_timeout(started, options.max_elapsed_ms, "extract fields") {
        return envelope;
    }

    let mut fields = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let mut compiled = 0usize;
    let mut only_blocked = true;
    let mut truncated_any = false;

    for (name, spec) in specs {
        if let Some(envelope) = budget_timeout(started, options.max_elapsed_ms, "extract fields") {
            return envelope;
        }
        if let Some(message) = invalid_name(name) {
            only_blocked = false;
            fields.insert(name.clone(), FieldValue::Null);
            diagnostics.push(diagnostic(name, FieldIssueKind::InvalidName, message));
            continue;
        }
        let attr = match normalized_attr(spec.attr.as_deref()) {
            Ok(attr) => attr,
            Err(message) => {
                only_blocked = false;
                fields.insert(name.clone(), FieldValue::Null);
                diagnostics.push(diagnostic(name, FieldIssueKind::InvalidAttribute, message));
                continue;
            }
        };
        if let Some((code, message)) = guard_selector(&spec.selector, options) {
            debug_assert_eq!(code, ErrorCode::Blocked);
            fields.insert(name.clone(), FieldValue::Null);
            diagnostics.push(diagnostic(name, FieldIssueKind::BlockedSelector, message));
            continue;
        }
        let css = match compile_selector(&spec.selector) {
            Ok(css) => css,
            Err(message) => {
                only_blocked = false;
                fields.insert(name.clone(), FieldValue::Null);
                diagnostics.push(diagnostic(name, FieldIssueKind::InvalidSelector, message));
                continue;
            }
        };
        compiled += 1;
        let Some(collected) = collect(&document, &css, attr, spec.all, options, started) else {
            return Envelope::fail(
                ErrorCode::Timeout,
                format!(
                    "extract fields exceeded wall-clock budget of {} ms",
                    options.max_elapsed_ms
                ),
                elapsed_ms(started),
            );
        };

        if collected.matched == 0 {
            diagnostics.push(diagnostic(
                name,
                FieldIssueKind::ZeroMatches,
                format!(
                    "selector {:?} matched 0 elements (valid empty result, not a parse failure)",
                    spec.selector
                ),
            ));
        }
        if collected.missing_attr > 0 {
            let attr_name = attr.unwrap_or_default();
            diagnostics.push(diagnostic(
                name,
                FieldIssueKind::MissingAttribute,
                format!(
                    "attribute {attr_name:?} is absent on {} matched element(s)",
                    collected.missing_attr
                ),
            ));
        }
        if collected.truncated {
            truncated_any = true;
            diagnostics.push(diagnostic(
                name,
                FieldIssueKind::Truncated,
                format!(
                    "all=true matched more than {} elements; array truncated to max_results",
                    options.max_results.max(1)
                ),
            ));
        }

        let value = if spec.all {
            FieldValue::List(collected.values)
        } else {
            match collected.values.into_iter().next().flatten() {
                Some(value) => FieldValue::Text(value),
                None => FieldValue::Null,
            }
        };
        fields.insert(name.clone(), value);
    }

    if compiled == 0 {
        let message = format!(
            "fields 全部非法，没有任何字段可执行：{}",
            summarize(&diagnostics)
        );
        let code = if only_blocked {
            ErrorCode::Blocked
        } else {
            ErrorCode::Parse
        };
        return Envelope::fail(code, message, elapsed_ms(started));
    }

    let conclusion = format!(
        "extracted {} field(s) from {} declared field(s); {} diagnostic(s)",
        fields.len(),
        specs.len(),
        diagnostics.len()
    );
    let mut result = FieldSetResult {
        fields,
        diagnostics,
        conclusion,
        stability: STABILITY_NOTE.to_owned(),
    };
    if !truncated_any {
        return Envelope::ok(result, Trust::UntrustedPageContent, elapsed_ms(started));
    }
    // 截断必须显式降级（PLAN §3.4「禁止静默降级」）：只有上限被触发时才走到这里，
    // data 仍然完整可用，只是至少一个 all=true 数组没有取全。
    result
        .conclusion
        .push_str("; at least one all=true array was truncated to max_results");
    Envelope::degraded(
        result,
        "fields-truncated",
        ErrorCode::Limit,
        elapsed_ms(started),
    )
}

/// 一次字段扫描的收集结果。
struct Collected {
    matched: usize,
    missing_attr: usize,
    values: Vec<Option<String>>,
    truncated: bool,
}

/// 手写节点推进（同 `select_matches`）：让零命中也经过预算检查，避免深链/大页面
/// 在选择器内部一次跑完、绕过 `max_elapsed_ms`。
fn collect(
    document: &scraper::Html,
    css: &scraper::Selector,
    attr: Option<&str>,
    all: bool,
    options: &ExtractOptions,
    started: Instant,
) -> Option<Collected> {
    let max_values = options.max_results.max(1);
    let mut collected = Collected {
        matched: 0,
        missing_attr: 0,
        values: Vec::new(),
        truncated: false,
    };
    for node in document.tree.nodes() {
        if elapsed_ms(started) >= options.max_elapsed_ms {
            return None;
        }
        let Some(element) = scraper::ElementRef::wrap(node) else {
            continue;
        };
        if element.parent().is_none() {
            continue;
        }
        if !css.matches(&element) {
            continue;
        }
        let value = match attr {
            Some(attr) => element.value().attr(attr).map(str::to_owned),
            None => Some(collapse_ws(&element.text().collect::<String>())),
        };
        collected.matched += 1;
        if value.is_none() {
            collected.missing_attr += 1;
        }
        if !all {
            collected.values.push(value);
            break;
        }
        if collected.values.len() >= max_values {
            collected.truncated = true;
            break;
        }
        collected.values.push(value);
    }
    Some(collected)
}

/// 声明级校验：空对象 / 超过字段数上限。返回 (code, message)。
fn validate_declaration(specs: &BTreeMap<String, FieldSpec>) -> Option<(ErrorCode, String)> {
    if specs.is_empty() {
        return Some((
            ErrorCode::Parse,
            "fields 不能为空对象：至少声明一个字段，例如 {\"title\":{\"selector\":\"h1\"}}"
                .to_owned(),
        ));
    }
    if specs.len() > MAX_FIELDS {
        return Some((
            ErrorCode::Parse,
            format!(
                "fields 最多声明 {MAX_FIELDS} 个字段，收到 {} 个",
                specs.len()
            ),
        ));
    }
    None
}

/// 字段名不合法时返回可读原因。
fn invalid_name(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Some("字段名不能为空".to_owned());
    };
    if !(first.is_alphabetic() || first == '_') {
        return Some(format!("字段名 {name:?} 非法：必须以字母或下划线开头"));
    }
    if name.chars().count() > MAX_FIELD_NAME_CHARS {
        return Some(format!(
            "字段名 {name:?} 非法：最多 {MAX_FIELD_NAME_CHARS} 个字符"
        ));
    }
    if let Some(bad) = chars.find(|ch| !(*ch).is_alphanumeric() && !matches!(ch, '_' | '-' | '.')) {
        return Some(format!(
            "字段名 {name:?} 非法：含不允许的字符 {bad:?}（只允许字母、数字、'_'、'-'、'.'）"
        ));
    }
    None
}

/// 校验并规范化 `attr`。`None` 表示取文本。
fn normalized_attr(attr: Option<&str>) -> Result<Option<&str>, String> {
    match attr {
        None => Ok(None),
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err("attr 不能是空字符串；取文本请省略 attr 或传 null".to_owned());
            }
            if let Some(bad) = trimmed
                .chars()
                .find(|ch| ch.is_control() || ch.is_whitespace())
            {
                return Err(format!("attr {trimmed:?} 非法：含空白或控制字符 {bad:?}"));
            }
            Ok(Some(trimmed))
        }
    }
}

fn diagnostic(field: &str, kind: FieldIssueKind, message: impl Into<String>) -> FieldDiagnostic {
    FieldDiagnostic {
        field: field.to_owned(),
        kind,
        message: message.into(),
    }
}

fn summarize(diagnostics: &[FieldDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|entry| format!("{} ({:?})", entry.field, entry.kind))
        .collect::<Vec<_>>()
        .join(", ")
}
