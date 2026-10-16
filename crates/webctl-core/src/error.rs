//! 结果信封错误码封闭集（PLAN v0.4 §3.4）。
//!
//! 封闭集的意义：调用方可以穷举分支，不存在的错误不允许凭空造码。
use serde::{Deserialize, Serialize};

/// 错误码全集（11 个），序列化为 SCREAMING_SNAKE_CASE 字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// 成功。
    Ok,
    /// 超时（网络或解析预算）。
    Timeout,
    /// 网络层失败：DNS / 连接 / TLS。
    Network,
    /// 被拒绝：HTTP 4xx/5xx、拦截页、robots 禁止。
    Blocked,
    /// URL 非法或协议不支持。
    BadUrl,
    /// 解析失败（与零命中严格区分）。
    Parse,
    /// 已降级执行，结果不完整；必须同时写 meta.degraded_from。
    Degraded,
    /// 能力不存在（如 JS 渲染、OCR）。是不支持，不是失败。
    Unsupported,
    /// 会话状态丢失或过期。
    SessionLost,
    /// 命中尺寸或条目上限，结果被截断。
    Limit,
    /// 内部错误（不应出现；出现即为缺陷）。
    Internal,
}

impl ErrorCode {
    /// 全集，顺序与 PLAN v0.4 §3.4 一致。
    pub const ALL: [ErrorCode; 11] = [
        Self::Ok,
        Self::Timeout,
        Self::Network,
        Self::Blocked,
        Self::BadUrl,
        Self::Parse,
        Self::Degraded,
        Self::Unsupported,
        Self::SessionLost,
        Self::Limit,
        Self::Internal,
    ];

    /// 稳定的线上字符串（与 serde 重命名一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Timeout => "TIMEOUT",
            Self::Network => "NETWORK",
            Self::Blocked => "BLOCKED",
            Self::BadUrl => "BAD_URL",
            Self::Parse => "PARSE",
            Self::Degraded => "DEGRADED",
            Self::Unsupported => "UNSUPPORTED",
            Self::SessionLost => "SESSION_LOST",
            Self::Limit => "LIMIT",
            Self::Internal => "INTERNAL",
        }
    }

    /// 成功码只有一个。
    pub fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }

    /// 降级类：出现时必须同时写 meta.degraded_from（禁止静默降级）。
    pub fn is_degradation(self) -> bool {
        matches!(self, Self::Degraded | Self::Unsupported | Self::Limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ENV-05：封闭集与线上字符串被冻结。
    #[test]
    fn code_set_and_strings_are_frozen() {
        let expected = [
            "OK",
            "TIMEOUT",
            "NETWORK",
            "BLOCKED",
            "BAD_URL",
            "PARSE",
            "DEGRADED",
            "UNSUPPORTED",
            "SESSION_LOST",
            "LIMIT",
            "INTERNAL",
        ];
        assert_eq!(ErrorCode::ALL.map(ErrorCode::as_str), expected);
        assert_eq!(ErrorCode::ALL.len(), 11);
        for code in ErrorCode::ALL {
            let value = serde_json::to_value(code).expect("serialize");
            assert_eq!(value, serde_json::json!(code.as_str()));
            let back: ErrorCode = serde_json::from_value(value).expect("deserialize");
            assert_eq!(back, code);
        }
    }

    /// ENV-05：未定义的码必须反序列化失败（v0.3 的码不得复活）。
    #[test]
    fn unknown_code_is_rejected() {
        for bogus in [
            "NO_BROWSER",
            "BROWSER_CRASH",
            "TOOL_MISSING",
            "STALE_REF",
            "WAT",
        ] {
            assert!(
                serde_json::from_value::<ErrorCode>(serde_json::json!(bogus)).is_err(),
                "{bogus} 不应被接受"
            );
        }
    }

    #[test]
    fn ok_and_degradation_are_disjoint() {
        assert!(ErrorCode::Ok.is_ok() && !ErrorCode::Ok.is_degradation());
        assert!(ErrorCode::Degraded.is_degradation());
        assert!(ErrorCode::Unsupported.is_degradation());
        assert!(ErrorCode::Limit.is_degradation());
        assert!(!ErrorCode::Timeout.is_degradation());
        assert!(!ErrorCode::Parse.is_degradation());
    }
}
