//! 礼貌取回层：robots、限速、退避与拦截识别（PLAN v0.4 §3.8 / ADR-0004）。
//!
//! 这是 `crate::transport` 的**组合层**：不改变取回语义，只在外面加四件事——
//! 1. 默认按 `robots.txt` 判定（可显式关闭，关闭事实会写进结果）；
//! 2. 每主机并发上限与最小请求间隔（[`throttle::HostGate`]）；
//! 3. 有上限的退避重试，优先尊重 `Retry-After`；
//! 4. 拦截页识别（保守特征），命中即 `BLOCKED`，**不重试轰炸**。
//!
//! 因此本层使用 `transport::fetch_raw`（4xx/5xx 不折叠为失败），以便读到错误响应头。
pub mod backoff;
pub mod detect;
pub mod robots;
pub mod throttle;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use url::Url;
use webctl_core::{Envelope, ErrorCode, Trust};

use crate::transport::{self, FetchOptions, Fetched};

pub use backoff::DEFAULT_MAX_BACKOFF;
pub use detect::Interception;
pub use robots::RobotsRules;
pub use throttle::{HostGate, ThrottleConfig};

/// 默认 User-Agent（诚实标识自己，不做伪装）。
pub const DEFAULT_USER_AGENT: &str = "webctl/0.1 (+https://github.com/Thatgfsj/webctl)";

/// robots.txt 的处置结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RobotsOutcome {
    /// 已检查且允许。
    Allowed,
    /// 已检查且禁止（此时不会发起请求）。
    Disallowed,
    /// 配置显式关闭了 robots 判定（事实记录在结果里）。
    Disabled,
    /// robots.txt 不可用（404/网络失败）——按行业惯例视为允许。
    Unavailable,
}

/// 礼貌层配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolitenessConfig {
    /// 限速（每主机并发与最小间隔）。
    pub throttle: ThrottleConfig,
    /// 最大重试次数（不含首次请求）。
    pub max_retries: u32,
    /// 指数退避基数。
    pub retry_base_delay: Duration,
    /// 退避上限。
    pub max_backoff: Duration,
    /// 是否遵守 robots.txt。
    pub respect_robots: bool,
    /// 用于 robots 组匹配的 User-Agent。
    pub user_agent: String,
    /// 是否启用拦截页识别。
    pub detect_interception: bool,
}

impl Default for PolitenessConfig {
    fn default() -> Self {
        Self {
            throttle: ThrottleConfig::default(),
            max_retries: 2,
            retry_base_delay: Duration::from_millis(500),
            max_backoff: DEFAULT_MAX_BACKOFF,
            respect_robots: true,
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            detect_interception: true,
        }
    }
}

/// 礼貌取回的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoliteFetched {
    /// 底层取回结果（状态、头、重定向链、正文）。
    pub fetched: Fetched,
    /// 实际发起的总次数（含重试）。
    pub attempts: u32,
    /// robots 处置结果。
    pub robots: RobotsOutcome,
    /// 是否发生过重试。
    pub retried: bool,
}

/// 礼貌取回器。
pub struct PoliteFetcher {
    config: PolitenessConfig,
    options: FetchOptions,
    gate: HostGate,
    robots_cache: Mutex<HashMap<String, Option<RobotsRules>>>,
}

impl PoliteFetcher {
    /// 用配置与底层取回选项构建。
    ///
    /// `options.user_agent` 若为空会被设为 `config.user_agent`，保证 robots 匹配与实际 UA 一致。
    pub fn new(config: PolitenessConfig, mut options: FetchOptions) -> Self {
        if options.user_agent.is_none() {
            options.user_agent = Some(config.user_agent.clone());
        }
        Self {
            gate: HostGate::new(config.throttle.clone()),
            config,
            options,
            robots_cache: Mutex::new(HashMap::new()),
        }
    }

    /// 配置。
    pub fn config(&self) -> &PolitenessConfig {
        &self.config
    }

    /// 底层取回选项。
    pub fn options(&self) -> &FetchOptions {
        &self.options
    }

    /// 取回 robots.txt 并解析（按 origin 缓存；`None` 表示不可用）。
    pub async fn robots_rules(&self, url: &Url) -> Option<RobotsRules> {
        let host = url.host_str().map(str::to_owned)?;
        // 必须带端口：否则非默认端口站点（例如测试用 127.0.0.1:随机端口）会去抓 80 端口。
        let origin = url.origin().ascii_serialization();
        {
            let cache = self.robots_cache.lock().await;
            if let Some(entry) = cache.get(&origin) {
                return entry.clone();
            }
        }

        let _permit = self.gate.acquire(&host).await;
        let robots_url = format!("{origin}/robots.txt");
        let rules = match transport::fetch(&robots_url, &self.options).await.data {
            Some(fetched) if fetched.status < 400 => {
                fetched.content.text.as_deref().map(RobotsRules::parse)
            }
            _ => None,
        };
        self.robots_cache.lock().await.insert(origin, rules.clone());
        rules
    }

    /// 礼貌地取回一个 URL。任何失败都返回带错误码的信封，绝不 panic。
    pub async fn fetch(&self, target: &str) -> Envelope<PoliteFetched> {
        let started = Instant::now();
        let elapsed = || started.elapsed().as_millis() as u64;

        let url = match validate_target(target) {
            Ok(url) => url,
            Err(message) => return Envelope::fail(ErrorCode::BadUrl, message, elapsed()),
        };
        let host = url.host_str().unwrap_or_default().to_owned();

        // 1) robots
        let robots = if self.config.respect_robots {
            match self.robots_rules(&url).await {
                Some(rules) => {
                    if robots::allows(&rules, &self.config.user_agent, url.path()) {
                        RobotsOutcome::Allowed
                    } else {
                        RobotsOutcome::Disallowed
                    }
                }
                None => RobotsOutcome::Unavailable,
            }
        } else {
            RobotsOutcome::Disabled
        };
        if robots == RobotsOutcome::Disallowed {
            return Envelope::fail(
                ErrorCode::Blocked,
                format!("robots.txt disallows {target}"),
                elapsed(),
            );
        }

        // 2) 限速 + 重试循环
        let mut attempt: u32 = 0;
        loop {
            let _permit = self.gate.acquire(&host).await;
            let envelope = transport::fetch_raw(target, &self.options).await;
            let attempts = attempt + 1;

            let fetched = match envelope.data {
                Some(fetched) => fetched,
                None => {
                    let retryable =
                        matches!(envelope.code, ErrorCode::Timeout | ErrorCode::Network);
                    if retryable && attempt < self.config.max_retries {
                        let delay = backoff::exponential(
                            attempt,
                            self.config.retry_base_delay,
                            self.config.max_backoff,
                        );
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    return Envelope::fail(envelope.code, envelope.error, elapsed());
                }
            };

            // 3) 拦截页识别（命中即失败，不重试）
            if self.config.detect_interception
                && let Some(hit) = detect::detect(
                    fetched.status,
                    &fetched.headers,
                    fetched.content.text.as_deref(),
                )
            {
                return Envelope::fail(
                    ErrorCode::Blocked,
                    format!("suspected interception for {target}: {}", hit.reason),
                    elapsed(),
                );
            }

            // 4) 429：优先 Retry-After，且重试有上限
            if fetched.status == 429 && attempt < self.config.max_retries {
                let retry_after = header_value(&fetched.headers, "retry-after");
                let delay = backoff::delay_for(
                    retry_after.as_deref(),
                    attempt,
                    self.config.retry_base_delay,
                    self.config.max_backoff,
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
                continue;
            }

            if fetched.status >= 400 {
                return Envelope::fail(
                    ErrorCode::Blocked,
                    format!("HTTP {} {target}", fetched.status),
                    elapsed(),
                );
            }

            let out = PoliteFetched {
                fetched,
                attempts,
                robots,
                retried: attempts > 1,
            };
            return if out.fetched.content.truncated {
                Envelope::degraded(out, "response-body-truncated", ErrorCode::Limit, elapsed())
            } else {
                Envelope::ok(out, Trust::UntrustedPageContent, elapsed())
            };
        }
    }
}

fn validate_target(target: &str) -> Result<Url, String> {
    let url = Url::parse(target).map_err(|error| format!("invalid url {target:?}: {error}"))?;
    match url.scheme() {
        "http" | "https" => {}
        other => return Err(format!("unsupported scheme {other:?}: only http/https")),
    }
    if url.host_str().is_none() {
        return Err(format!("url has no host: {target}"));
    }
    Ok(url)
}

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_targets_without_network() {
        assert!(validate_target("").is_err());
        assert!(validate_target("file:///etc/passwd").is_err());
        assert!(validate_target("https://").is_err());
        assert!(validate_target("http://example.test/").is_ok());
    }

    #[test]
    fn defaults_follow_the_politeness_adr() {
        let config = PolitenessConfig::default();
        assert_eq!(config.throttle.max_concurrency_per_host, 2);
        assert_eq!(config.throttle.min_interval, Duration::from_secs(1));
        assert!(config.respect_robots);
        assert!(config.detect_interception);
        assert_eq!(config.max_retries, 2);
        assert!(config.max_backoff <= DEFAULT_MAX_BACKOFF);
    }

    #[test]
    fn user_agent_is_wired_into_options() {
        let fetcher = PoliteFetcher::new(PolitenessConfig::default(), FetchOptions::default());
        assert_eq!(
            fetcher.options().user_agent.as_deref(),
            Some(DEFAULT_USER_AGENT)
        );
    }
}
