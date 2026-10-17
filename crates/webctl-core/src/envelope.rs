//! 结果信封与降级语义（PLAN v0.4 §3.4）。
//!
//! 两条硬规则：
//! 1. 页面或网络内容一律 trust = untrusted_page_content。
//! 2. 禁止静默降级：降级必须同时写 code 与 meta.degraded_from；
//!    构造器成对写入，validate() 兜底校验。
use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;

/// 执行引擎。v0.4 只有内置一种；保留字段是为了将来可选的内置 JS 引擎。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    /// 进程内实现，零外部进程、零外部浏览器。
    #[default]
    Builtin,
}

impl Engine {
    /// 线上字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
        }
    }
}

/// 信任标记。默认取最不可信的一侧（失败关闭）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
    /// 页面或网络内容，固定使用该值。
    #[default]
    UntrustedPageContent,
    /// webctl 自己生成的本地数据（如 caps 报告）。
    TrustedLocal,
}

/// 降级时的**可行动信息**（议题 #115）。
///
/// 目的：降级不只告诉调用方"我降级了"，还要说清**为什么**与**还能试什么**，
/// 否则 agent 只能猜。`reason` 是机器可读的原因码；`detected` 是观察到的证据；
/// `next_steps` 是**真实存在的工具名**（由 MCP 侧的测试反查保证）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actionable {
    /// 机器可读的原因码（例如 `no-server-rendered-content`）。
    pub reason: String,
    /// 观察到的证据（容器、指纹、量化信号），逐条可核对。
    pub detected: Vec<String>,
    /// 建议尝试的通道（工具名）。空数组表示"没有别的可试"。
    pub next_steps: Vec<String>,
}

/// 信封元信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    /// 执行引擎。
    pub engine: Engine,
    /// 信任标记。
    pub trust: Trust,
    /// 降级来源：能力或路径名（例如 js-render）。未降级时为 null。
    pub degraded_from: Option<String>,
    /// 降级时的可行动信息（#115）。未降级时**不出现**在 JSON 里。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actionable: Option<Actionable>,
    /// 结果是否来自过期数据。v0.4 无跨请求引用，恒为 false，字段保留供将来使用。
    pub stale: bool,
    /// 端到端耗时（毫秒）。
    pub elapsed_ms: u64,
}

/// 信封不变量被破坏。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeViolation {
    /// ok 为真却带了错误码。
    OkWithoutOkCode,
    /// ok 为假但 code 仍是 OK。
    FailureWithOkCode,
    /// 失败却没有错误说明。
    FailureWithoutError,
    /// 降级码却没有 degraded_from（静默降级）。
    SilentDegradation,
    /// 有 degraded_from 却没有降级码。
    ReasonWithoutDegradation,
    /// 失败还带了 data。
    FailureWithData,
    /// 未降级却带了可行动信息（actionable 只在降级时有意义）。
    ActionableWithoutDegradation,
}

/// 统一结果信封：ok / data / error / code / meta。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope<T> {
    /// 是否成功。
    pub ok: bool,
    /// 成功时的数据；失败固定为 null。
    pub data: Option<T>,
    /// 失败说明；成功为空字符串。
    pub error: String,
    /// 错误码。
    pub code: ErrorCode,
    /// 元信息。
    pub meta: Meta,
}

impl<T> Envelope<T> {
    fn meta(trust: Trust, degraded_from: Option<String>, elapsed_ms: u64) -> Meta {
        Meta {
            engine: Engine::Builtin,
            trust,
            degraded_from,
            actionable: None,
            stale: false,
            elapsed_ms,
        }
    }

    /// 成功且未降级。页面或网络内容必须传 Trust::UntrustedPageContent。
    pub fn ok(data: T, trust: Trust, elapsed_ms: u64) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: String::new(),
            code: ErrorCode::Ok,
            meta: Self::meta(trust, None, elapsed_ms),
        }
    }

    /// 失败。error 必须非空。
    pub fn fail(code: ErrorCode, error: impl Into<String>, elapsed_ms: u64) -> Self {
        Self {
            ok: false,
            data: None,
            error: error.into(),
            code,
            meta: Self::meta(Trust::UntrustedPageContent, None, elapsed_ms),
        }
    }

    /// 降级执行：data 仍可用，但必须显式记录来源与原因码。
    pub fn degraded(data: T, from: impl Into<String>, code: ErrorCode, elapsed_ms: u64) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: String::new(),
            code,
            meta: Self::meta(Trust::UntrustedPageContent, Some(from.into()), elapsed_ms),
        }
    }

    /// 附加降级时的可行动信息（#115）。链式调用，便于在 `Envelope::degraded` 之后补。
    pub fn with_actionable(mut self, actionable: Actionable) -> Self {
        self.meta.actionable = Some(actionable);
        self
    }

    /// 校验信封不变量。重点是禁止静默降级。
    pub fn validate(&self) -> Result<(), EnvelopeViolation> {
        use EnvelopeViolation as Violation;
        if self.ok && !self.code.is_ok() && !self.code.is_degradation() {
            return Err(Violation::OkWithoutOkCode);
        }
        if !self.ok && self.code.is_ok() {
            return Err(Violation::FailureWithOkCode);
        }
        if !self.ok && self.error.is_empty() {
            return Err(Violation::FailureWithoutError);
        }
        if !self.ok && self.data.is_some() {
            return Err(Violation::FailureWithData);
        }
        match (
            self.meta.degraded_from.as_deref(),
            self.code.is_degradation(),
        ) {
            (None, true) => return Err(Violation::SilentDegradation),
            (Some(_), false) => return Err(Violation::ReasonWithoutDegradation),
            _ => {}
        }
        // #115：可行动信息只在降级时有意义；干净的 OK 带上它属于协议误用。
        if self.meta.actionable.is_some() && self.meta.degraded_from.is_none() {
            return Err(Violation::ActionableWithoutDegradation);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ENV-01 / ENV-03：成功信封字段与 trust 标记。
    #[test]
    fn ok_envelope_shape_and_trust() {
        let env = Envelope::ok(serde_json::json!({"a": 1}), Trust::UntrustedPageContent, 12);
        assert!(env.ok);
        assert_eq!(env.code, ErrorCode::Ok);
        assert!(env.error.is_empty());
        assert!(env.meta.degraded_from.is_none());
        assert!(!env.meta.stale);
        assert_eq!(env.meta.elapsed_ms, 12);
        assert_eq!(env.meta.engine.as_str(), "builtin");
        assert_eq!(env.validate(), Ok(()));
        let value = serde_json::to_value(&env).expect("serialize");
        assert_eq!(value["meta"]["trust"], "untrusted_page_content");
        assert_eq!(value["meta"]["engine"], "builtin");
    }

    /// ENV-02：失败信封不带 data。
    #[test]
    fn failure_envelope_shape() {
        let env: Envelope<serde_json::Value> =
            Envelope::fail(ErrorCode::BadUrl, "bad url: no scheme", 3);
        assert!(!env.ok);
        assert!(env.data.is_none());
        assert!(!env.error.is_empty());
        assert_eq!(env.validate(), Ok(()));
        let value = serde_json::to_value(&env).expect("serialize");
        assert_eq!(value["code"], "BAD_URL");
        assert!(value["data"].is_null());
    }

    /// ENV-04：降级必须带 degraded_from；两者缺一即视为静默降级。
    #[test]
    fn degradation_must_be_explicit() {
        let good = Envelope::degraded(
            serde_json::json!({"quotes": []}),
            "js-render",
            ErrorCode::Degraded,
            42,
        );
        assert_eq!(good.meta.degraded_from.as_deref(), Some("js-render"));
        assert_eq!(good.validate(), Ok(()));

        let mut silent = good.clone();
        silent.meta.degraded_from = None;
        assert_eq!(silent.validate(), Err(EnvelopeViolation::SilentDegradation));
        assert_eq!(
            serde_json::to_value(&silent).unwrap()["meta"]["degraded_from"],
            serde_json::Value::Null
        );

        let mut lying = Envelope::ok(1u8, Trust::TrustedLocal, 1);
        lying.meta.degraded_from = Some("js-render".to_owned());
        assert_eq!(
            lying.validate(),
            Err(EnvelopeViolation::ReasonWithoutDegradation)
        );
    }

    /// ENV-08：未知字段容忍（向前兼容）。
    #[test]
    fn unknown_fields_are_tolerated() {
        let raw = serde_json::json!({
            "ok": true,
            "data": {"x": 1},
            "error": "",
            "code": "OK",
            "future_field": [1, 2, 3],
            "meta": {
                "engine": "builtin",
                "trust": "trusted_local",
                "degraded_from": null,
                "stale": false,
                "elapsed_ms": 1,
                "future_meta": "whatever"
            }
        });
        let env: Envelope<serde_json::Value> = serde_json::from_value(raw).expect("tolerant");
        assert_eq!(env.validate(), Ok(()));
    }

    /// ENV-11：engine 的未定义取值必须反序列化失败。
    #[test]
    fn unknown_engine_is_rejected() {
        assert!(serde_json::from_value::<Engine>(serde_json::json!("quantum")).is_err());
        assert!(serde_json::from_value::<Engine>(serde_json::json!("builtin")).is_ok());
    }
}
