//! 运行配置、四层优先级与配置文件装载（PLAN v0.4 §3.3；TESTING CLI-01…CLI-12）。
//!
//! 优先级（高 → 低）：**CLI 参数 > 环境变量 > 配置文件 > 内置默认**。
//!
//! 本模块只负责“配置语义、合并与配置文件解析”，不联网、不落盘、不启动子进程。
//! 会话状态目录的文件锁 / TTL 属议题 #34（PR 05）；路径解析本身**不在这里**，
//! `default_state_dir()` 只是 crate::state::resolve_state_dir 的薄委托，全仓
//! 只有 state::dir 一处路径拼装。
//!
//! 代理语义（CLI-03）：
//! - 只有**显式**配置才走代理：WEBCTL_PROXY 环境变量、配置文件 proxy、CLI --proxy。
//! - **不读宿主 HTTP_PROXY / HTTPS_PROXY / ALL_PROXY**：未配置时必须直连，
//!   否则开发机/CI 上的环境代理会劫持离线测试（webctl-web 侧 None 即 no_proxy）。
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;

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

impl OutputFormat {
    /// 稳定的线上字符串（与 serde 重命名一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Text => "text",
        }
    }
}

/// 运行配置（合并后的最终结果）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// 会话与状态目录（仅路径；锁与 TTL 见议题 #34）。
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
    /// 显式 HTTP(S) 代理 URL；None 表示直连（不读宿主代理环境变量）。
    ///
    /// 可能带 user:password@ 凭据，任何对外输出都必须先经 Config::redacted。
    pub proxy: Option<String>,
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
            proxy: None,
        }
    }
}

impl Config {
    /// 去掉代理 URL 中凭据后的副本，供 config 等命令输出（CLI-10）。
    pub fn redacted(&self) -> Self {
        let mut clone = self.clone();
        if let Some(proxy) = clone.proxy.take() {
            clone.proxy = Some(redact_url_userinfo(&proxy));
        }
        clone
    }
}

/// 单个配置层：字段为 None 表示“本层未给出”，继续沿用低优先级的值。
///
/// 只有本议题（PR 10）验收涉及的字段进入配置层：输出格式、请求预算、离线、代理。
/// 配置文件用 serde 反序列化（deny_unknown_fields，拼错键必须报错）；
/// 环境变量与 CLI 参数由代码显式构造。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigLayer {
    /// 输出格式。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<OutputFormat>,
    /// 请求预算（毫秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// 离线模式。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline: Option<bool>,
    /// 代理 URL。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
}

impl ConfigLayer {
    /// 把本层显式给出的值覆盖到 config；未给出的字段保持原值。
    pub fn apply_to(&self, config: &mut Config) {
        if let Some(value) = self.output {
            config.output = value;
        }
        if let Some(value) = self.timeout_ms {
            config.timeout_ms = value;
        }
        if let Some(value) = self.offline {
            config.offline = value;
        }
        if let Some(value) = &self.proxy {
            config.proxy = Some(value.clone());
        }
    }

    /// 从环境变量构造本层（CLI-01 第二优先级）。
    ///
    /// 取值用注入的 get 闭包完成，便于测试完全离线、不受宿主环境干扰。
    /// 值非法时返回明确错误，而不是静默忽略（禁止静默降级）。
    pub fn from_env<F>(get: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut layer = Self::default();
        if let Some(raw) = env_value(&get, ENV_OUTPUT) {
            layer.output = Some(parse_output(ENV_OUTPUT, &raw)?);
        }
        if let Some(raw) = env_value(&get, ENV_TIMEOUT_MS) {
            layer.timeout_ms = Some(parse_u64(ENV_TIMEOUT_MS, &raw)?);
        }
        if let Some(raw) = env_value(&get, ENV_OFFLINE) {
            layer.offline = Some(parse_bool(ENV_OFFLINE, &raw)?);
        }
        // 只有 webctl 专用变量才开启代理；宿主 HTTP(S)_PROXY 一律忽略（见模块文档）。
        if let Some(raw) = env_value(&get, ENV_PROXY)
            && !raw.trim().is_empty()
        {
            layer.proxy = Some(raw);
        }
        Ok(layer)
    }
}

/// 合并三层覆盖到内置默认（CLI-01）：file 最低、env 居中、cli 最高。
pub fn resolve(file: &ConfigLayer, env: &ConfigLayer, cli: &ConfigLayer) -> Config {
    let mut config = Config::default();
    file.apply_to(&mut config);
    env.apply_to(&mut config);
    cli.apply_to(&mut config);
    config
}

/// 配置文件路径约定（CLI-02）：
/// $WEBCTL_CONFIG > $XDG_CONFIG_HOME/webctl/config.toml
/// > $HOME/.config/webctl/config.toml（Windows 用 $USERPROFILE）> ./webctl.toml。
pub fn default_config_path<F>(get: F) -> PathBuf
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(path) = env_value(&get, ENV_CONFIG)
        && !path.trim().is_empty()
    {
        return PathBuf::from(path);
    }
    if let Some(xdg) = env_value(&get, "XDG_CONFIG_HOME")
        && !xdg.trim().is_empty()
    {
        return PathBuf::from(xdg).join("webctl").join("config.toml");
    }
    if let Some(home) = env_value(&get, "HOME").or_else(|| env_value(&get, "USERPROFILE")) {
        return PathBuf::from(home)
            .join(".config")
            .join("webctl")
            .join("config.toml");
    }
    PathBuf::from("webctl.toml")
}

/// 读取并解析 TOML 配置文件（CLI-02）。
///
/// 读取失败与语法/字段错误分别给出可读说明；调用方负责映射到非零退出码。
pub fn load_config_file(path: &Path) -> Result<ConfigLayer, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|error| ConfigError::Read {
        path: path.to_path_buf(),
        source: error.to_string(),
    })?;
    toml::from_str::<ConfigLayer>(&text).map_err(|error| ConfigError::Parse {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

/// 默认状态目录：**委托** `crate::state::resolve_state_dir(None)`，全仓只有
/// state::dir 一处路径拼装（消除历史上这里的第二份解析）。
///
/// 优先级（高 → 低）：显式参数（CLI --state-dir 语义） > `WEBCTL_STATE_DIR` >
/// `$HOME/.webctl`（Windows 回退 `$USERPROFILE`） > `./.webctl`。
/// 空值语义：`WEBCTL_STATE_DIR=""`、空的显式参数、空的 HOME 一律视为未设置，
/// 逐层回退，**绝不返回空路径**。文件锁、TTL 与持久化属议题 #34，见 crate::state。
pub fn default_state_dir() -> PathBuf {
    crate::state::resolve_state_dir(None)
}

/// 环境变量名：配置文件路径。
pub const ENV_CONFIG: &str = "WEBCTL_CONFIG";
/// 环境变量名：状态目录（路径语义；持久化属议题 #34）。
pub const ENV_STATE_DIR: &str = "WEBCTL_STATE_DIR";
/// 环境变量名：输出格式（json / text）。
pub const ENV_OUTPUT: &str = "WEBCTL_OUTPUT";
/// 环境变量名：请求预算（毫秒）。
pub const ENV_TIMEOUT_MS: &str = "WEBCTL_TIMEOUT_MS";
/// 环境变量名：离线开关（1/true/yes/on）。
pub const ENV_OFFLINE: &str = "WEBCTL_OFFLINE";
/// 环境变量名：显式代理 URL。
pub const ENV_PROXY: &str = "WEBCTL_PROXY";

/// webctl 读取的全部环境变量（CLI-11：必须在 --help 与文档中列出）。
///
/// 注意不包含 HTTP_PROXY / HTTPS_PROXY / ALL_PROXY：宿主代理不会被采用。
pub const ENV_VARS: [&str; 9] = [
    ENV_CONFIG,
    ENV_STATE_DIR,
    ENV_OUTPUT,
    ENV_TIMEOUT_MS,
    ENV_OFFLINE,
    ENV_PROXY,
    "XDG_CONFIG_HOME",
    "HOME",
    "USERPROFILE",
];

/// 配置装载错误。退出码统一映射见 crates/webctl/src/cli.rs 的 exit_code。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// 环境变量取值非法。
    Env {
        /// 变量名。
        key: String,
        /// 原始取值。
        value: String,
        /// 期望格式说明。
        expected: &'static str,
    },
    /// 配置文件读取失败。
    Read {
        /// 文件路径。
        path: PathBuf,
        /// 底层错误说明。
        source: String,
    },
    /// 配置文件语法或字段错误。
    Parse {
        /// 文件路径。
        path: PathBuf,
        /// 解析器给出的可读说明。
        message: String,
    },
}

impl ConfigError {
    /// 映射到封闭错误码；配置问题统一归为 PARSE（退出码 6）。
    pub fn code(&self) -> ErrorCode {
        ErrorCode::Parse
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env {
                key,
                value,
                expected,
            } => write!(f, "环境变量 {key}={value:?} 非法：期望 {expected}"),
            Self::Read { path, source } => {
                write!(f, "读取配置文件 {} 失败：{source}", path.display())
            }
            Self::Parse { path, message } => {
                write!(f, "配置文件 {} 解析失败：{message}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// 去掉 URL 中的 userinfo（凭据），保留 scheme / host / path（CLI-10 脱敏）。
///
/// 不引入 URL 依赖：只在 authority 段内定位最后一个 @；没有 scheme:// 时
/// 也按同一规则处理，避免 user:pass@host 这种裸写法泄漏。
pub fn redact_url_userinfo(raw: &str) -> String {
    let prefix_len = match raw.find("://") {
        Some(index) => index + 3,
        None => 0,
    };
    let rest = &raw[prefix_len..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    match authority.rfind('@') {
        Some(at) => format!("{}***{}", &raw[..prefix_len], &rest[at..]),
        None => raw.to_owned(),
    }
}

fn env_value<F>(get: &F, key: &str) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    get(key).or_else(|| get(&key.to_ascii_lowercase()))
}

fn parse_output(key: &str, raw: &str) -> Result<OutputFormat, ConfigError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "json" => Ok(OutputFormat::Json),
        "text" => Ok(OutputFormat::Text),
        _ => Err(ConfigError::Env {
            key: key.to_owned(),
            value: raw.to_owned(),
            expected: "json | text",
        }),
    }
}

fn parse_bool(key: &str, raw: &str) -> Result<bool, ConfigError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::Env {
            key: key.to_owned(),
            value: raw.to_owned(),
            expected: "true/false（或 1/0、yes/no、on/off）",
        }),
    }
}

fn parse_u64(key: &str, raw: &str) -> Result<u64, ConfigError> {
    raw.trim().parse::<u64>().map_err(|_| ConfigError::Env {
        key: key.to_owned(),
        value: raw.to_owned(),
        expected: "非负整数",
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    fn temp_path(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "webctl-config-{tag}-{}-{nanos}.toml",
            std::process::id()
        ))
    }

    /// 默认值保持保守。
    #[test]
    fn defaults_are_conservative() {
        let config = Config::default();
        assert!(!config.offline, "默认不离线");
        assert_eq!(config.max_concurrency_per_host, 2, "默认每主机并发 2");
        assert_eq!(config.min_interval_ms, 1_000);
        assert_eq!(config.timeout_ms, 30_000);
        assert_eq!(config.output, OutputFormat::Json);
        assert_eq!(config.proxy, None, "默认不使用代理");
        assert!(config.state_dir.is_absolute() || config.state_dir.ends_with(".webctl"));
    }

    /// CLI-04：线上字符串被冻结。
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
        assert_eq!(OutputFormat::Json.as_str(), "json");
        assert_eq!(OutputFormat::Text.as_str(), "text");
    }

    /// CLI-01：四层优先级，逐层覆盖。
    #[test]
    fn layers_override_lowest_to_highest() {
        let file = ConfigLayer {
            timeout_ms: Some(11),
            output: Some(OutputFormat::Text),
            ..ConfigLayer::default()
        };
        let env_layer = ConfigLayer {
            timeout_ms: Some(22),
            ..ConfigLayer::default()
        };
        let cli = ConfigLayer {
            timeout_ms: Some(33),
            ..ConfigLayer::default()
        };

        let with_file = resolve(&file, &ConfigLayer::default(), &ConfigLayer::default());
        assert_eq!(with_file.timeout_ms, 11);
        assert_eq!(with_file.output, OutputFormat::Text);
        let with_env = resolve(&file, &env_layer, &ConfigLayer::default());
        assert_eq!(with_env.timeout_ms, 22);
        assert_eq!(with_env.output, OutputFormat::Text);
        let with_cli = resolve(&file, &env_layer, &cli);
        assert_eq!(with_cli.timeout_ms, 33);
        assert_eq!(with_cli.output, OutputFormat::Text);
        let defaults = resolve(
            &ConfigLayer::default(),
            &ConfigLayer::default(),
            &ConfigLayer::default(),
        );
        assert_eq!(defaults.timeout_ms, 30_000, "四层都缺省 → 内置默认");
        assert_eq!(defaults.output, OutputFormat::Json);
        assert_eq!(defaults.proxy, None);
    }

    /// CLI-01：环境层解析本议题涉及的字段。
    #[test]
    fn env_layer_parses_values() {
        let layer = ConfigLayer::from_env(env(&[
            ("WEBCTL_OUTPUT", "text"),
            ("WEBCTL_TIMEOUT_MS", "1234"),
            ("WEBCTL_OFFLINE", "yes"),
        ]))
        .expect("env layer");
        assert_eq!(layer.output, Some(OutputFormat::Text));
        assert_eq!(layer.timeout_ms, Some(1234));
        assert_eq!(layer.offline, Some(true));
        assert_eq!(layer.proxy, None);
    }

    /// CLI-03：代理只认 WEBCTL_PROXY；宿主 HTTP(S)_PROXY 必须被忽略（直连）。
    #[test]
    fn proxy_only_comes_from_webctl_proxy() {
        let dedicated =
            ConfigLayer::from_env(env(&[("WEBCTL_PROXY", "http://127.0.0.1:8081")])).expect("env");
        assert_eq!(dedicated.proxy.as_deref(), Some("http://127.0.0.1:8081"));

        let lowercase =
            ConfigLayer::from_env(env(&[("webctl_proxy", "http://127.0.0.1:8084")])).expect("env");
        assert_eq!(lowercase.proxy.as_deref(), Some("http://127.0.0.1:8084"));

        for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy"] {
            let host = ConfigLayer::from_env(env(&[(key, "http://host-proxy:3128")]))
                .unwrap_or_else(|error| panic!("{key}: {error}"));
            assert_eq!(host.proxy, None, "{key} 不得被 webctl 采用");
        }

        let empty = ConfigLayer::from_env(env(&[("WEBCTL_PROXY", "  ")])).expect("env");
        assert_eq!(empty.proxy, None, "空白代理等价于未配置");
    }

    /// CLI-02 / CLI-11：非法环境变量必须明确报错，而不是被忽略。
    #[test]
    fn env_layer_rejects_invalid_values() {
        for (key, value) in [
            ("WEBCTL_OUTPUT", "yaml"),
            ("WEBCTL_TIMEOUT_MS", "soon"),
            ("WEBCTL_OFFLINE", "maybe"),
        ] {
            let error = ConfigLayer::from_env(env(&[(key, value)])).expect_err("must fail");
            assert!(matches!(error, ConfigError::Env { .. }), "{key}");
            assert!(error.to_string().contains(key));
            assert_eq!(error.code(), ErrorCode::Parse);
        }
    }

    /// CLI-02：配置文件路径约定。
    #[test]
    fn config_path_convention() {
        assert_eq!(
            default_config_path(env(&[("WEBCTL_CONFIG", "/etc/webctl.toml")])),
            PathBuf::from("/etc/webctl.toml")
        );
        assert_eq!(
            default_config_path(env(&[("XDG_CONFIG_HOME", "/xdg")])),
            PathBuf::from("/xdg/webctl/config.toml")
        );
        assert_eq!(
            default_config_path(env(&[("HOME", "/home/u")])),
            PathBuf::from("/home/u/.config/webctl/config.toml")
        );
        assert_eq!(default_config_path(env(&[])), PathBuf::from("webctl.toml"));
    }

    /// CLI-02：合法 TOML 被解析进配置层。
    #[test]
    fn config_file_parses_toml() {
        let path = temp_path("valid");
        std::fs::write(
            &path,
            "output = \"text\"\ntimeout_ms = 42\noffline = true\nproxy = \"http://127.0.0.1:9\"\n",
        )
        .expect("write");
        let layer = load_config_file(&path).expect("parse");
        let _ = std::fs::remove_file(&path);
        assert_eq!(layer.output, Some(OutputFormat::Text));
        assert_eq!(layer.timeout_ms, Some(42));
        assert_eq!(layer.offline, Some(true));
        assert_eq!(layer.proxy.as_deref(), Some("http://127.0.0.1:9"));
    }

    /// CLI-02：语法错误与拼错的键都要有可读错误（含文件路径）。
    #[test]
    fn config_file_errors_are_reported() {
        let syntax = temp_path("syntax");
        std::fs::write(&syntax, "output = [\n").expect("write");
        let error = load_config_file(&syntax).expect_err("syntax must fail");
        let _ = std::fs::remove_file(&syntax);
        assert!(matches!(error, ConfigError::Parse { .. }));
        assert!(error.to_string().contains("解析失败"));
        assert!(error.to_string().contains("syntax"));
        assert_eq!(error.code(), ErrorCode::Parse);

        let typo = temp_path("typo");
        std::fs::write(&typo, "outpt = \"json\"\n").expect("write");
        let error = load_config_file(&typo).expect_err("unknown field must fail");
        let _ = std::fs::remove_file(&typo);
        assert!(matches!(error, ConfigError::Parse { .. }), "{error}");

        let missing = temp_path("missing");
        let error = load_config_file(&missing).expect_err("missing must fail");
        assert!(matches!(error, ConfigError::Read { .. }));
        assert!(error.to_string().contains("读取"));
    }

    /// 禁止未知字段（serde 侧，配置文件走同一类型）。
    #[test]
    fn config_layer_denies_unknown_fields() {
        let error = serde_json::from_value::<ConfigLayer>(serde_json::json!({
            "timeout_ms": 5,
            "tmeout_ms": 6
        }));
        assert!(error.is_err());
    }

    /// CLI-10：代理凭据必须可脱敏。
    #[test]
    fn proxy_credentials_are_redacted() {
        assert_eq!(
            redact_url_userinfo("http://user:s3cret@proxy.local:8080"),
            "http://***@proxy.local:8080"
        );
        assert_eq!(
            redact_url_userinfo("https://alice@proxy.local/path?x=1"),
            "https://***@proxy.local/path?x=1"
        );
        assert_eq!(
            redact_url_userinfo("user:s3cret@proxy.local:8080"),
            "***@proxy.local:8080"
        );
        assert_eq!(
            redact_url_userinfo("http://proxy.local:8080"),
            "http://proxy.local:8080"
        );
        assert_eq!(redact_url_userinfo("not a url"), "not a url");

        let config = Config {
            proxy: Some("http://user:s3cret@proxy.local:8080".to_owned()),
            ..Config::default()
        };
        let redacted = config.redacted();
        assert!(!redacted.proxy.unwrap().contains("s3cret"));
    }
}
