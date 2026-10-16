//! 退避与 `Retry-After`（PLAN v0.4 §3.8）。
//!
//! 只接受 `Retry-After` 的**秒数**形式（HTTP-date 形式退回指数退避，并在文档里写明）；
//! 退避一律有上限，**不允许无限重试**。
use std::time::Duration;

/// 退避上限默认值。
pub const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(30);

/// 解析 `Retry-After` 的秒数形式；HTTP-date 形式返回 `None`（调用方改用指数退避）。
///
/// 非法、负数按 `None` 处理，绝不返回巨大的等待时间。
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    let seconds: u64 = value.trim().parse().ok()?;
    if seconds > 3600 {
        // 超过 1 小时的上游要求视为异常，交给调用方按退避上限处理
        return None;
    }
    Some(Duration::from_secs(seconds))
}

/// 第 `attempt` 次重试（从 0 计）的指数退避：`base * 2^attempt`，封顶 `cap`。
pub fn exponential(attempt: u32, base: Duration, cap: Duration) -> Duration {
    let factor = 1u32.checked_shl(attempt.min(16)).unwrap_or(u32::MAX);
    base.saturating_mul(factor).min(cap)
}

/// 综合决定这次等待多久：优先 `Retry-After`，否则指数退避，两者都不超过 `cap`。
pub fn delay_for(
    retry_after: Option<&str>,
    attempt: u32,
    base: Duration,
    cap: Duration,
) -> Duration {
    let from_header = retry_after
        .and_then(parse_retry_after)
        .map(|value| value.min(cap));
    from_header.unwrap_or_else(|| exponential(attempt, base, cap))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_seconds_form_only() {
        assert_eq!(parse_retry_after("2"), Some(Duration::from_secs(2)));
        assert_eq!(parse_retry_after(" 0 "), Some(Duration::from_secs(0)));
        assert_eq!(parse_retry_after("abc"), None);
        assert_eq!(parse_retry_after("-1"), None);
        assert_eq!(parse_retry_after("7200"), None, "异常大的值由退避上限接管");
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"),
            None,
            "HTTP-date 形式不解析"
        );
    }

    #[test]
    fn exponential_grows_and_is_capped() {
        let base = Duration::from_millis(100);
        let cap = Duration::from_millis(800);
        assert_eq!(exponential(0, base, cap), Duration::from_millis(100));
        assert_eq!(exponential(1, base, cap), Duration::from_millis(200));
        assert_eq!(exponential(2, base, cap), Duration::from_millis(400));
        assert_eq!(exponential(3, base, cap), Duration::from_millis(800));
        assert_eq!(exponential(9, base, cap), cap, "永远封顶");
        assert_eq!(exponential(40, base, cap), cap, "超大 attempt 不溢出");
    }

    #[test]
    fn retry_after_wins_but_respects_cap() {
        let base = Duration::from_millis(100);
        let cap = Duration::from_secs(5);
        assert_eq!(delay_for(Some("3"), 0, base, cap), Duration::from_secs(3));
        assert_eq!(
            delay_for(Some("60"), 0, base, cap),
            cap,
            "上游要求超过上限时按上限等"
        );
        assert_eq!(delay_for(None, 1, base, cap), Duration::from_millis(200));
        assert_eq!(
            delay_for(Some("nonsense"), 2, base, cap),
            Duration::from_millis(400),
            "非法 header 退回指数退避"
        );
    }
}
