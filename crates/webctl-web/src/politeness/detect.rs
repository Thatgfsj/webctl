//! 拦截页识别（PLAN v0.4 §3.8 / TESTING.md POLITE-09）。
//!
//! 目标：**把"被拦截"从"抓到一页奇怪内容"里认出来**，返回明确原因，交给调用方归 `BLOCKED`。
//! 只做保守特征匹配——宁可漏判（交给调用方按 4xx/5xx 处理），也不要把正常页面误判成拦截。
use serde::{Deserialize, Serialize};

/// 拦截页特征（保守集合）。
const CHALLENGE_MARKERS: [&str; 7] = [
    "cf-chl",
    "just a moment",
    "attention required",
    "checking your browser",
    "enable javascript and cookies to continue",
    "captcha",
    "验证码",
];

/// 拦截判定结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interception {
    /// 人类可读的原因。
    pub reason: String,
}

/// 判定一个响应是否是拦截页。`None` 表示"看不出被拦截"。
pub fn detect(
    status: u16,
    headers: &[(String, String)],
    body: Option<&str>,
) -> Option<Interception> {
    let server = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("server"))
        .map(|(_, value)| value.to_ascii_lowercase())
        .unwrap_or_default();
    let body_lower = body.map(str::to_ascii_lowercase).unwrap_or_default();

    let marker = CHALLENGE_MARKERS
        .iter()
        .find(|marker| body_lower.contains(**marker));
    let cloudflare_server = server.contains("cloudflare");

    match (marker, cloudflare_server, status) {
        (Some(marker), _, _) => Some(Interception {
            reason: format!("命中拦截页特征 {marker:?}（HTTP {status}）"),
        }),
        (None, true, 403 | 429 | 503) => Some(Interception {
            reason: format!("Cloudflare 边缘返回 HTTP {status}，判定为拦截"),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn detects_cloudflare_challenge_pages() {
        let found = detect(
            503,
            &headers(&[("Server", "cloudflare")]),
            Some("<html><title>Just a moment...</title><div id=\"cf-chl\"></div></html>"),
        );
        assert!(found.is_some(), "应当识别为拦截");
        assert!(found.unwrap().reason.contains("拦截"));
    }

    #[test]
    fn detects_captcha_and_chinese_marker() {
        assert!(detect(200, &headers(&[]), Some("Please solve the CAPTCHA")).is_some());
        assert!(detect(200, &headers(&[]), Some("请完成验证码后继续")).is_some());
    }

    #[test]
    fn cloudflare_edge_errors_without_marker_are_still_interception() {
        assert!(detect(403, &headers(&[("server", "cloudflare")]), Some("denied")).is_some());
        assert!(detect(503, &headers(&[("server", "cloudflare")]), None).is_some());
        assert!(
            detect(403, &headers(&[("server", "nginx")]), Some("forbidden")).is_none(),
            "普通 403 不当作拦截（由调用方按 4xx 处理）"
        );
    }

    #[test]
    fn normal_pages_are_not_flagged() {
        let normal =
            "<html><body>Hello, this is a normal page about clouds and flare.</body></html>";
        assert!(detect(200, &headers(&[("server", "nginx")]), Some(normal)).is_none());
        assert!(detect(200, &headers(&[]), None).is_none());
    }
}
