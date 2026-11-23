//! `extract` 工具的 fields 接线（议题 #193）。
//!
//! 单独成模块：dispatch.rs 已接近 1000 行上限（仓库硬规则），与 `embedded_tool` /
//! `web_search_tool` 同构。
//!
//! 分工边界：
//! - 本模块只做 MCP 参数 → web 层类型的转换、三条路径的路由、以及结果合并；
//! - 真正的抽取语义（选择器编译、字段名校验、诊断码、预算）全部在
//!   `webctl_web::extract::fields`，这里**不重复校验**，避免两处规则漂移。
//!
//! 零回归约束：只给 `selector` 时逐字走原路径（`extract` / `extract_body`），
//! 输出与引入 `fields` 之前完全相同（tests/extract_fields.rs 有对照用例）。

use std::collections::BTreeMap;

use serde_json::Value;
use webctl_core::ErrorCode;
use webctl_web::extract::{self, ExtractOptions, FieldSpec};
use webctl_web::transport::Body;

use crate::dispatch::{fail, to_value};
use crate::params::ExtractParams;

/// 既没给 `selector` 也没给 `fields` 时的统一报错文案（只在这里维护一份）。
pub(crate) const MISSING_TARGET: &str = "selector 与 fields 至少给一个；extract 需要其中之一";

/// 把 MCP 参数层的字段声明转成 web 层类型（校验在 web 层做，失败进 diagnostics）。
pub(crate) fn field_specs(params: &ExtractParams) -> Option<BTreeMap<String, FieldSpec>> {
    params.fields.as_ref().map(|fields| {
        fields
            .iter()
            .map(|(name, spec)| {
                (
                    name.clone(),
                    FieldSpec {
                        selector: spec.selector.clone(),
                        attr: spec.attr.clone(),
                        all: spec.all,
                    },
                )
            })
            .collect()
    })
}

/// 在已解析的 HTML 文本上执行 extract：选择器模式、fields 模式或两者合并。
pub(crate) fn extract_value(
    html: &str,
    selector: Option<&str>,
    fields: Option<&BTreeMap<String, FieldSpec>>,
    options: &ExtractOptions,
) -> Value {
    match (selector, fields) {
        (Some(selector), None) => to_value(extract::extract(html, selector, options)),
        (None, Some(fields)) => to_value(extract::extract_fields(html, fields, options)),
        (Some(selector), Some(fields)) => merge_fields(
            to_value(extract::extract(html, selector, options)),
            to_value(extract::extract_fields(html, fields, options)),
        ),
        // tool_extract 已提前拦下，这里只是防御性兜底（措辞与提前拦截共用一份）。
        (None, None) => fail(ErrorCode::Parse, MISSING_TARGET),
    }
}

/// 按内容类型路由 extract（非 HTML 走 UNSUPPORTED；fields 同语义）。
pub(crate) fn extract_body_value(
    body: &Body,
    selector: Option<&str>,
    fields: Option<&BTreeMap<String, FieldSpec>>,
    options: &ExtractOptions,
) -> Value {
    match (selector, fields) {
        (Some(selector), None) => to_value(extract::extract_body(body, selector, options)),
        (None, Some(fields)) => to_value(extract::extract_fields_body(body, fields, options)),
        (Some(selector), Some(fields)) => merge_fields(
            to_value(extract::extract_body(body, selector, options)),
            to_value(extract::extract_fields_body(body, fields, options)),
        ),
        (None, None) => fail(ErrorCode::Parse, MISSING_TARGET),
    }
}

/// 选择器结果 + fields 结果合并成一个信封。
///
/// 失败优先：选择器自身的失败 -> 原样返回（不吞掉 MATCH 路径的真实原因）；
/// fields 的整体失败（空 schema / 全部字段非法 / 超时）-> 返回 fields 失败信封，
/// 因为用户显式要了字段，静默丢掉更糟。两者都成功时把 `fields` 与 `diagnostics`
/// 并入 `data`，原有 `matches` / `truncated` 等字段**一个都不改**。
fn merge_fields(mut base: Value, fields: Value) -> Value {
    let base_ok = base.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if !base_ok {
        return base;
    }
    let fields_ok = fields.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if !fields_ok {
        return fields;
    }
    let Some(field_data) = fields.get("data").filter(|data| data.is_object()) else {
        return base;
    };
    let Some(data) = base.get_mut("data").filter(|data| data.is_object()) else {
        return base;
    };
    if let (Some(target), Some(source)) = (data.as_object_mut(), field_data.as_object()) {
        for key in ["fields", "diagnostics"] {
            if let Some(value) = source.get(key) {
                target.insert(key.to_owned(), value.clone());
            }
        }
    }
    carry_fields_degradation(&mut base, &fields);
    base
}

/// 把 fields 一侧的降级码搬到合并信封上。
///
/// 只在 selector 一侧是干净 OK 时搬：selector 自己已经降级时（例如命中数被
/// `max-results` 截断）保留它自己的来源码——fields 的截断仍留在 `diagnostics` 里，
/// 不能用第二个原因码覆盖掉第一个。
fn carry_fields_degradation(base: &mut Value, fields: &Value) {
    let base_is_ok = base.get("code").and_then(Value::as_str) == Some("OK");
    let fields_code = fields.get("code").and_then(Value::as_str).unwrap_or("OK");
    if !base_is_ok || fields_code == "OK" {
        return;
    }
    base["code"] = Value::String(fields_code.to_owned());
    if let Some(error) = fields.get("error") {
        base["error"] = error.clone();
    }
    // degraded_from 与 actionable 必须成对搬：信封自检禁止「降级码没有来源」。
    let Some(source) = fields.get("meta").and_then(Value::as_object) else {
        return;
    };
    let Some(meta) = base.get_mut("meta").and_then(Value::as_object_mut) else {
        return;
    };
    for key in ["degraded_from", "actionable"] {
        if let Some(value) = source.get(key) {
            meta.insert(key.to_owned(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 合并成功：字段并入 data，原有 matches 原样保留。
    #[test]
    fn merge_keeps_selector_data_and_adds_fields() {
        let base =
            json!({"ok": true, "code": "OK", "data": {"matches": [1, 2], "truncated": false}});
        let fields = json!({"ok": true, "data": {"fields": {"title": "T"}, "diagnostics": []}});
        let merged = merge_fields(base, fields);
        assert_eq!(merged["data"]["matches"], json!([1, 2]));
        assert_eq!(merged["data"]["truncated"], json!(false));
        assert_eq!(merged["data"]["fields"]["title"], "T");
        assert_eq!(merged["data"]["diagnostics"], json!([]));
    }

    /// 选择器路径失败时保持原样（不把 fields 的成功结果拼到失败信封上）。
    #[test]
    fn merge_keeps_selector_failure() {
        let base = json!({"ok": false, "code": "PARSE", "error": "bad selector"});
        let fields = json!({"ok": true, "data": {"fields": {"title": "T"}, "diagnostics": []}});
        let merged = merge_fields(base, fields);
        assert_eq!(merged["ok"], false);
        assert_eq!(merged["code"], "PARSE");
        assert!(merged["data"].is_null());
    }

    /// fields 整体失败时取 fields 失败信封（用户显式要的字段不能静默丢掉）。
    #[test]
    fn merge_prefers_fields_failure() {
        let base = json!({"ok": true, "data": {"matches": []}});
        let fields = json!({"ok": false, "code": "BLOCKED", "error": "all fields blocked"});
        let merged = merge_fields(base, fields);
        assert_eq!(merged["ok"], false);
        assert_eq!(merged["code"], "BLOCKED");
    }

    /// fields 一侧截断（LIMIT）时，合并结果必须保留降级码与来源，不能被 selector 的 OK 吞掉。
    #[test]
    fn merge_carries_fields_degradation() {
        let base = json!({"ok": true, "code": "OK", "error": "", "data": {"matches": [1]},
            "meta": {"degraded_from": null, "actionable": null}});
        let fields = json!({"ok": true, "code": "LIMIT", "error": "",
            "data": {"fields": {"tags": ["a"]}, "diagnostics": []},
            "meta": {"degraded_from": "fields-truncated", "actionable": {"reason": "field-limit-reached",
                "detected": [], "next_steps": ["extract"]}}});
        let merged = merge_fields(base, fields);
        assert_eq!(merged["code"], "LIMIT");
        assert_eq!(merged["meta"]["degraded_from"], "fields-truncated");
        assert_eq!(
            merged["meta"]["actionable"]["reason"],
            "field-limit-reached"
        );
        assert_eq!(merged["data"]["matches"], json!([1]));
        assert_eq!(merged["data"]["fields"]["tags"], json!(["a"]));
    }

    /// 两侧都降级时保留 selector 自己的来源码（fields 的截断仍在 diagnostics 里）。
    #[test]
    fn merge_keeps_selector_reason_when_both_degraded() {
        let base = json!({"ok": true, "code": "LIMIT", "error": "", "data": {"matches": []},
            "meta": {"degraded_from": "max-results"}});
        let fields = json!({"ok": true, "code": "LIMIT", "error": "",
            "data": {"fields": {}, "diagnostics": []},
            "meta": {"degraded_from": "fields-truncated"}});
        let merged = merge_fields(base, fields);
        assert_eq!(merged["code"], "LIMIT");
        assert_eq!(merged["meta"]["degraded_from"], "max-results");
    }

    /// data 不是对象时不做任何拼接（防御：信封形状变化不至于 panic）。
    #[test]
    fn merge_tolerates_non_object_data() {
        let merged = merge_fields(
            json!({"ok": true, "data": null}),
            json!({"ok": true, "data": {"fields": {"title": "T"}}}),
        );
        assert!(merged["data"].is_null());
    }
}
