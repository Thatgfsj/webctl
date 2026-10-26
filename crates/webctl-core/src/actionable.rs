//! 降级原因码表与可行动信息（议题 #115 / #124）。
//!
//! **单一来源**：`Envelope::degraded` 在构造时按 `degraded_from` 自动补 `meta.actionable`。
//! 新增一条降级路径必须在这里登记（给出 reason + next_steps），或显式登记进 `NO_CHANNEL`
//! （表示"由上层自带精确 actionable"，不是漏登记）。`crates/webctl-web/tests/degrade_reasons.rs`
//! 会扫描源码里的 `degraded_from` 字面量做机械核对。
//!
//! 纪律：`next_steps` 里只能是**真实存在的工具名**（`check-skill-tools` 与 MCP 侧反查用例守护）。

use crate::{Actionable, Envelope};

/// 一条降级路径的可行动计划。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// 机器可读原因码。
    pub reason: &'static str,
    /// 建议尝试的通道（真实工具名；空数组表示没有别的通道）。
    pub next_steps: &'static [&'static str],
}

/// 已登记的降级来源 → 计划（`degraded_from` 精确匹配，组合串按逗号分段）。
pub const PLANS: &[(&str, Plan)] = &[
    (
        "max-results",
        Plan {
            reason: "result-limit-reached",
            next_steps: &["paginate"],
        },
    ),
    (
        "char-budget",
        Plan {
            reason: "result-budget-exhausted",
            next_steps: &["extract", "doc_text"],
        },
    ),
    (
        "scan-cap",
        Plan {
            reason: "scan-cap-reached",
            next_steps: &["extract"],
        },
    ),
    (
        "response-body-truncated",
        Plan {
            reason: "response-truncated",
            next_steps: &["download"],
        },
    ),
    (
        "pagination-max-pages",
        Plan {
            reason: "page-limit-reached",
            next_steps: &["paginate", "session"],
        },
    ),
    (
        "pagination-loop",
        Plan {
            reason: "pagination-loop-detected",
            next_steps: &["session"],
        },
    ),
    (
        "pagination-fetch-failed",
        Plan {
            reason: "page-fetch-failed",
            next_steps: &["session", "paginate"],
        },
    ),
    (
        "channel-fetch-failed",
        Plan {
            reason: "feed-unavailable",
            next_steps: &["fetch"],
        },
    ),
    (
        "pagination-degraded-page",
        Plan {
            reason: "page-degraded",
            next_steps: &["session"],
        },
    ),
    (
        "input-size-limit",
        Plan {
            reason: "input-too-large",
            next_steps: &["doc_text", "download"],
        },
    ),
    (
        "json-input-size-limit",
        Plan {
            reason: "input-too-large",
            next_steps: &["json_query"],
        },
    ),
    (
        "pdf-input-size-limit",
        Plan {
            reason: "input-too-large",
            next_steps: &["download"],
        },
    ),
    (
        "use-json-query",
        Plan {
            reason: "use-json-query",
            next_steps: &["json_query"],
        },
    ),
    (
        "format-unknown",
        Plan {
            reason: "unknown-format",
            next_steps: &["doc_text", "json_query"],
        },
    ),
    (
        "pdf-no-text-layer",
        Plan {
            reason: "document-has-no-text-layer",
            next_steps: &["embedded_data", "download"],
        },
    ),
    (
        "pdf-encrypted",
        Plan {
            reason: "document-encrypted",
            next_steps: &[],
        },
    ),
    (
        "pdf-no-pages",
        Plan {
            reason: "document-has-no-pages",
            next_steps: &[],
        },
    ),
    (
        "table-span-clamped",
        Plan {
            reason: "table-span-clamped",
            next_steps: &[],
        },
    ),
    (
        "dom_smoothie",
        Plan {
            reason: "article-extractor-fallback",
            next_steps: &["extract", "tables"],
        },
    ),
    (
        "transport:raw-bytes-unavailable",
        Plan {
            reason: "content-not-text",
            next_steps: &["download"],
        },
    ),
];

/// 动态前缀：`content-type:pdf` 这类形态按前缀匹配。
pub const PREFIX_PLANS: &[(&str, Plan)] = &[(
    "content-type:",
    Plan {
        reason: "content-type-unhandled",
        next_steps: &["doc_text", "download"],
    },
)];

/// 明确登记为「没有别的通道可试」或「由上层自带 actionable」的降级来源。
pub const NO_CHANNEL: &[&str] = &[
    // 由 degrade::guard 自带完整 actionable（reason/detected/next_steps）。
    "js-render",
    "js-render-failed",
    // 分页聚合来源（pagination-*）已登记进 PLANS；这里只剩"由上层自带精确 actionable"的。

    // 由 resolve_site 工具自带（reason 随层数变化）。
    "resolve-site",
];

/// 按 `degraded_from` 找计划：组合串（`a,b`）逐段匹配，先命中先用。
pub fn plan_for(from: &str) -> Option<Plan> {
    // 组合来源有两种拼法：逗号（extract 的截断原因）与加号（session 的 from+limit）。
    for token in from.split([',', '+']).map(str::trim) {
        if let Some((_, plan)) = PLANS.iter().find(|(key, _)| *key == token) {
            return Some(*plan);
        }
        if let Some((_, plan)) = PREFIX_PLANS
            .iter()
            .find(|(prefix, _)| token.starts_with(*prefix))
        {
            return Some(*plan);
        }
    }
    None
}

/// 所有已登记（有计划或显式无通道）的降级来源，供测试与文档核对。
pub fn registered() -> Vec<&'static str> {
    let mut all: Vec<&'static str> = PLANS.iter().map(|(key, _)| *key).collect();
    all.extend(NO_CHANNEL.iter().copied());
    all.extend(PREFIX_PLANS.iter().map(|(key, _)| *key));
    all.sort_unstable();
    all
}

/// 所有计划里用到的原因码（供测试区分"原因码"与"降级来源"）。
pub fn reasons() -> Vec<&'static str> {
    let mut all: Vec<&'static str> = PLANS
        .iter()
        .map(|(_, plan)| plan.reason)
        .chain(PREFIX_PLANS.iter().map(|(_, plan)| plan.reason))
        .collect();
    all.sort_unstable();
    all.dedup();
    all
}

/// 给已降级的信封补 `meta.actionable`（已有则不覆盖；`NO_CHANNEL` 不补）。
pub fn attach<T>(mut envelope: Envelope<T>) -> Envelope<T> {
    let Some(from) = envelope.meta.degraded_from.clone() else {
        return envelope;
    };
    if envelope.meta.actionable.is_some() {
        return envelope;
    }
    if NO_CHANNEL.contains(&from.as_str()) {
        return envelope;
    }
    let Some(plan) = plan_for(&from) else {
        return envelope;
    };
    envelope.meta.actionable = Some(Actionable {
        reason: plan.reason.to_owned(),
        detected: vec![format!("degraded_from: {from}")],
        next_steps: plan
            .next_steps
            .iter()
            .map(|step| (*step).to_owned())
            .collect(),
    });
    envelope
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 组合串按段匹配：`char-budget,max-results` 取第一个有计划的段。
    #[test]
    fn combined_from_matches_any_token() {
        assert_eq!(
            plan_for("char-budget,max-results").map(|plan| plan.reason),
            Some("result-budget-exhausted")
        );
        assert_eq!(
            plan_for("max-results").map(|plan| plan.reason),
            Some("result-limit-reached")
        );
        assert!(
            plan_for("js-render").is_none(),
            "由 guard 自带的来源不在表内"
        );
    }

    /// 加号组合（引擎聚合分页时用 `from+limit`）：按段匹配到已知来源。
    #[test]
    fn plus_separated_from_matches_any_token() {
        assert_eq!(
            plan_for("pagination-loop+js-render").map(|plan| plan.reason),
            Some("pagination-loop-detected")
        );
        assert_eq!(
            plan_for("js-render+pagination-max-pages").map(|plan| plan.reason),
            Some("page-limit-reached")
        );
    }

    /// 动态前缀：`content-type:pdf` 这类形态按前缀命中。
    #[test]
    fn prefix_rules_cover_dynamic_sources() {
        assert_eq!(
            plan_for("content-type:pdf").map(|plan| plan.reason),
            Some("content-type-unhandled")
        );
    }

    /// attach：给已降级、无 actionable、非 NO_CHANNEL 的信封补信息。
    #[test]
    fn attach_fills_actionable_but_respects_existing_and_no_channel() {
        let mut base = Envelope::ok(1u8, crate::Trust::TrustedLocal, 1);
        base.meta.degraded_from = Some("max-results".to_owned());
        let filled = attach(base);
        let actionable = filled.meta.actionable.expect("should attach");
        assert_eq!(actionable.reason, "result-limit-reached");
        assert_eq!(actionable.next_steps, vec!["paginate".to_owned()]);
        assert_eq!(
            actionable.detected,
            vec!["degraded_from: max-results".to_owned()]
        );

        let already = attach(Envelope::degraded(
            serde_json::json!({}),
            "max-results",
            crate::ErrorCode::Limit,
            1,
        ));
        assert_eq!(
            already.meta.actionable.expect("kept").reason,
            "result-limit-reached"
        );

        let mut guarded = Envelope::ok(2u8, crate::Trust::TrustedLocal, 1);
        guarded.meta.degraded_from = Some("js-render".to_owned());
        assert!(attach(guarded).meta.actionable.is_none(), "NO_CHANNEL 不补");
    }

    /// 登记表自检：键唯一、reason 非空、通道名非空（真实工具名由 MCP 侧用例反查）。
    #[test]
    fn registry_is_well_formed() {
        let mut keys: Vec<&str> = registered();
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "登记表不得有重复键");
        for (key, plan) in PLANS {
            assert!(!key.is_empty(), "键不得为空");
            assert!(!plan.reason.is_empty(), "{key}: reason 不得为空");
            for step in plan.next_steps {
                assert!(!step.is_empty(), "{key}: 通道名不得为空");
                assert!(
                    step.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'),
                    "{key}: 通道名必须是小写工具名（{step}）"
                );
            }
        }
    }
}
