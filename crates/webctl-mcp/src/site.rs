//! `resolve_site` 工具（0.2 M1 / ADR-0006）：站点名 -> 入口候选。
//!
//! 刻意放在独立模块而不是塞进 `dispatch.rs`：派发表已经 900+ 行，仓库规则要求
//! 单文件 < 1000 行；这里只放这一个工具的实现与用例。
//!
//! 诚实性（ADR-0006）：
//! - 永远给**候选列表**（绝不单选）；
//! - 只有命中内置映射（95 分）才算"可靠定位"；仅靠直推（≤55 分）或零命中时
//!   **显式降级**并给 `actionable.next_steps`；
//! - 不认识的名称返回空候选（不猜）。

use serde_json::{Value, json};
use webctl_core::{Actionable, Envelope, ErrorCode, Trust};
use webctl_web::resolve_site::resolve_l1;

use crate::dispatch::{fail, parse, to_value};
use crate::params::ResolveSiteParams;

/// 可靠定位阈值：内置映射 95 分，直推最高 55 分，60 分及以上才算可靠。
const CONFIDENT_SCORE: u8 = 60;

/// 默认候选数。
const DEFAULT_MAX_CANDIDATES: usize = 5;
/// 候选数上限（避免调用方要 1000 条）。
const MAX_CANDIDATES_CAP: usize = 10;

/// 站点名 -> 入口候选（L1 层：内置映射 + 域名直推，**不联网**）。
pub(crate) async fn tool_resolve_site(args: &Value) -> Value {
    let params = match parse::<ResolveSiteParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    let site = params.site.trim();
    if site.is_empty() {
        return fail(ErrorCode::Parse, "site 不能为空");
    }
    if site.contains("://") {
        return fail(
            ErrorCode::BadUrl,
            format!("resolve_site 只接受站点名，不接受 URL：{site}（已有 URL 请直接调用 fetch）"),
        );
    }
    let max_candidates = params
        .max_candidates
        .unwrap_or(DEFAULT_MAX_CANDIDATES)
        .clamp(1, MAX_CANDIDATES_CAP);
    let candidates = resolve_l1(site, max_candidates);
    let best = candidates
        .iter()
        .map(|candidate| candidate.score)
        .max()
        .unwrap_or(0);
    let confident = best >= CONFIDENT_SCORE;
    let data = json!({
        "query": site,
        "candidates": candidates,
        "confidence": if confident { "high" } else if candidates.is_empty() { "none" } else { "low" },
        "channel_note": "builtin=内置映射表（可靠）；direct=按常见 TLD 直推（猜测，需自行验证）",
    });
    if confident {
        // 本地推理结果，不是页面内容。
        return to_value(Envelope::ok(data, Trust::TrustedLocal, 0));
    }
    // A3：低置信/零命中是降级，不伪装成"定位成功"。
    // `Envelope::degraded` 固定 Trust::UntrustedPageContent —— 这里**保守**标记为不可信
    // 内容（候选是我们拼的，调用方不应把它当指令）。
    let (reason, detected) = if candidates.is_empty() {
        (
            "resolve-site-no-match",
            vec![
                format!("内置映射表未收录「{site}」"),
                "名称不是 ASCII 域名候选，未做任何猜测".to_owned(),
            ],
        )
    } else {
        (
            "resolve-site-low-confidence",
            vec![
                format!("仅命中直推候选，最高置信分 {best} < {CONFIDENT_SCORE}"),
                "直推按常见 TLD 拼装，域名可能属于他人或根本不存在".to_owned(),
            ],
        )
    };
    to_value(
        Envelope::degraded(data, "resolve-site-l1", ErrorCode::Degraded, 0).with_actionable(
            Actionable {
                reason: reason.to_owned(),
                detected,
                next_steps: vec!["fetch".to_owned(), "search".to_owned()],
            },
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内置命中：高分 + 可信 + **没有** actionable（干净的成功信封）。
    #[tokio::test]
    async fn builtin_hit_is_confident_and_not_degraded() {
        let value = tool_resolve_site(&json!({"site": "知乎"})).await;
        assert_eq!(value["ok"], true);
        assert_eq!(value["code"], "OK");
        assert_eq!(value["data"]["confidence"], "high");
        assert_eq!(value["data"]["candidates"][0]["url"], "https://zhihu.com");
        assert_eq!(value["data"]["candidates"][0]["channel"], "builtin");
        assert_eq!(value["data"]["candidates"][0]["score"], 95);
        assert!(
            value["meta"]["actionable"].is_null(),
            "可靠定位不该带 actionable：{value}"
        );
    }

    /// 只有直推：显式降级 + actionable（原因码与证据都要能被核对）。
    #[tokio::test]
    async fn direct_only_is_degraded_with_actionable() {
        let value = tool_resolve_site(&json!({"site": "example"})).await;
        assert_eq!(value["ok"], true);
        assert_eq!(value["code"], "DEGRADED");
        assert_eq!(value["data"]["confidence"], "low");
        assert_eq!(value["data"]["candidates"][0]["url"], "https://example.com");
        assert_eq!(value["meta"]["degraded_from"], "resolve-site-l1");
        let actionable = &value["meta"]["actionable"];
        assert_eq!(actionable["reason"], "resolve-site-low-confidence");
        assert!(
            !actionable["detected"]
                .as_array()
                .expect("detected")
                .is_empty(),
            "要带证据：{actionable}"
        );
        assert_eq!(actionable["next_steps"], json!(["fetch", "search"]));
    }

    /// 不认识的中文名：空候选（不猜）+ 显式降级 + actionable。
    #[tokio::test]
    async fn unknown_name_returns_empty_and_degraded() {
        let value = tool_resolve_site(&json!({"site": "某个不知名的中文站"})).await;
        assert_eq!(value["ok"], true);
        assert_eq!(value["code"], "DEGRADED");
        assert_eq!(value["data"]["confidence"], "none");
        assert_eq!(value["data"]["candidates"], json!([]));
        assert_eq!(
            value["meta"]["actionable"]["reason"],
            "resolve-site-no-match"
        );
    }

    /// 传 URL 是误用：直接 BAD_URL，并指向正确的工具。
    #[tokio::test]
    async fn url_argument_is_bad_url() {
        let value = tool_resolve_site(&json!({"site": "https://zhihu.com"})).await;
        assert_eq!(value["ok"], false);
        assert_eq!(value["code"], "BAD_URL");
        assert!(
            value["error"]
                .as_str()
                .is_some_and(|text| text.contains("fetch")),
            "错误要指出改用哪个工具：{value}"
        );
    }

    /// 空名字是参数错误，不是"没找到"。
    #[tokio::test]
    async fn empty_site_is_parse_error() {
        let value = tool_resolve_site(&json!({"site": "   "})).await;
        assert_eq!(value["ok"], false);
        assert_eq!(value["code"], "PARSE");
    }

    /// max_candidates 必须被夹到 [1, 10]：要 100 条不能真给 100 条。
    #[tokio::test]
    async fn max_candidates_is_clamped() {
        let many = tool_resolve_site(&json!({"site": "example", "max_candidates": 100})).await;
        let count = many["data"]["candidates"]
            .as_array()
            .expect("candidates")
            .len();
        assert!(count <= MAX_CANDIDATES_CAP, "上限 10 条：{count}");
        let one = tool_resolve_site(&json!({"site": "example", "max_candidates": 0})).await;
        assert_eq!(
            one["data"]["candidates"]
                .as_array()
                .expect("candidates")
                .len(),
            1,
            "0 也要至少 1 条"
        );
    }
}
