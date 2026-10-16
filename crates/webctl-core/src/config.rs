//! 运行配置（PLAN v0.4 §3.3）。
//!
//! 配置优先级（CLI 参数 > 环境变量 > 配置文件 > 默认）在 PR 10 落地；
//! 本 PR 只定义结构与默认值。
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 输出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    /// 机器可读（默认）。
    #[default]
    Json,
    /// 人类可读。
    Text,
}

/// 运行配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// 会话与状态目录。
    pub state_dir: PathBuf,
    /// 输出格式。
    pub output: OutputFormat,
    /// 单次网络请求预算（毫秒）。
    pub timeout_ms: u64,
    /// 离线模式：为真时不允许任何网络访问。
    pub offline: bool,
    /// 每主机并发上限（礼貌爬取的默认值，PLAN v0.4 §3.8）。
    pub max_concurrency_per_host: u16,
    /// 同主机最小请求间隔（毫秒）。
    pub min_interval_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            state_dir: default_state_dir(),
            output: OutputFormat::Json,
            timeout_ms: 30_000,
            offline: false,
            max_concurrency_per_host: 2,
            min_interval_ms: 1_000,
        }
    }
}

/// 默认状态目录：优先 WEBCTL_STATE_DIR，其次用户主目录下的 .webctl。
pub fn default_state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("WEBCTL_STATE_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        return PathBuf::from(home).join(".webctl");
    }
    PathBuf::from(".webctl")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_conservative() {
        let config = Config::default();
        assert!(!config.offline, "默认不离线");
        assert_eq!(config.max_concurrency_per_host, 2, "默认每主机并发 2");
        assert_eq!(config.min_interval_ms, 1_000);
        assert_eq!(config.timeout_ms, 30_000);
        assert_eq!(config.output, OutputFormat::Json);
        assert!(config.state_dir.is_absolute() || config.state_dir.ends_with(".webctl"));
    }

    #[test]
    fn output_format_strings_are_stable() {
        assert_eq!(
            serde_json::to_value(OutputFormat::Json).unwrap(),
            serde_json::json!("json")
        );
        assert_eq!(
            serde_json::to_value(OutputFormat::Text).unwrap(),
            serde_json::json!("text")
        );
    }
}
