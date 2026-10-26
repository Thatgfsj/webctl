//! 上游降级继承（#125）：以 URL 为输入的工具不得静默丢弃 fetch 的降级判定。
//!
//! doc_text / json_query / download 都自己重建信封：若只取上游文本、丢掉 meta，
//! 就会在 JS 空壳页上返回 OK（违反 A3）。本模块只做「把上游判定搬过来」，
//! 不判断页面是否依赖 JS（那是 fetch 的活）。

use serde_json::{Value, json};
use webctl_core::{Actionable, Envelope, ErrorCode};

/// 上游取回信封里的降级判定（#125）。
///
/// doc_text / json_query / download 都以 URL 为输入、自己重建信封：
/// 若只取上游的文本而丢掉 meta，就会在 JS 空壳页上静默返回 OK（A3 违规）。
#[derive(Debug, Clone)]
pub(crate) struct UpstreamDegrade {
    /// 降级来源标签（例如 js-render）。
    pub(crate) from: String,
    /// 降级码（DEGRADED / UNSUPPORTED / …）；只继承降级码，不继承失败。
    pub(crate) code: ErrorCode,
    /// 上游的可读说明（guard 写的是判定理由）。
    pub(crate) error: String,
    /// 上游的可行动信息（原因码 + 证据 + 建议通道）。
    pub(crate) actionable: Option<Actionable>,
}

impl UpstreamDegrade {
    /// 从取回信封里取下降级信息；未降级或失败时返回 None。
    pub(crate) fn capture<T>(envelope: &Envelope<T>) -> Option<Self> {
        if !envelope.ok {
            return None;
        }
        let from = envelope.meta.degraded_from.clone()?;
        Some(Self {
            from,
            code: if envelope.code.is_degradation() {
                envelope.code
            } else {
                ErrorCode::Degraded
            },
            error: envelope.error.clone(),
            actionable: envelope.meta.actionable.clone(),
        })
    }
}

/// 把上游降级如实写进本工具的信封（#125）：data 保持本工具结果，code/meta 用上游判定。
///
/// 只在**本工具结果本身是 OK** 时继承：本工具自己的失败（例如 json_query 解析失败）
/// 或已降级的情况保持原状——不把失败伪装成降级，也不覆盖更具体的原因。
pub(crate) fn apply_upstream_degradation(
    mut envelope: Value,
    upstream: Option<&UpstreamDegrade>,
) -> Value {
    let Some(upstream) = upstream else {
        return envelope;
    };
    let ok = envelope.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let code_is_ok = envelope.get("code").and_then(Value::as_str) == Some("OK");
    if !ok || !code_is_ok {
        return envelope;
    }
    envelope["code"] = serde_json::to_value(upstream.code).unwrap_or_else(|_| json!("DEGRADED"));
    envelope["error"] = json!(upstream.error);
    if let Some(meta) = envelope.get_mut("meta") {
        meta["degraded_from"] = json!(upstream.from);
        if let Some(actionable) = &upstream.actionable {
            meta["actionable"] = serde_json::to_value(actionable).unwrap_or(Value::Null);
        }
    }
    envelope
}
