//! MCP 调用上下文：配置 + 会话绑定。

use std::path::{Path, PathBuf};

use webctl_core::Config;

use crate::ownership::SessionBinding;

/// 一次 MCP server 生命周期内共享的不可变上下文。
#[derive(Debug, Clone)]
pub struct McpContext {
    /// 与 CLI 相同的生效配置（state dir、代理、超时、offline…）。
    pub config: Config,
    /// 会话归属绑定。
    pub binding: SessionBinding,
    /// 宿主注入的浏览器上下文（0.2 M0 / ADR-0006）。
    ///
    /// 来源：环境变量 `WEBCTL_BROWSER_CONTEXT` 指向的 JSON 文件（见 `browser_context_from_env`）。
    /// **空上下文 = 不注入任何头**；凭据类头跨源不携带（见 transport / engine 的去重与 SEC-06）。
    pub browser_context: Option<webctl_web::browser_context::BrowserContext>,
}

impl McpContext {
    /// 用配置与 owner 构造。
    pub fn new(config: Config, owner: Option<String>) -> Self {
        Self {
            config,
            binding: SessionBinding::new(owner),
            browser_context: browser_context_from_env(),
        }
    }

    /// 未绑定的上下文（测试或独立运行）。
    pub fn unbound(config: Config) -> Self {
        Self {
            config,
            binding: SessionBinding::unbound(),
            browser_context: browser_context_from_env(),
        }
    }

    /// 与 CLI 共享的 state dir（唯一来源是 core 的配置解析，不自行拼路径）。
    pub fn state_dir(&self) -> &Path {
        &self.config.state_dir
    }

    /// state dir 下的会话存储。
    pub fn session_store(&self) -> webctl_web::session::SessionStore {
        webctl_web::session::SessionStore::new(self.state_dir().to_path_buf())
    }

    /// 取回选项：超时/代理沿用配置。
    pub fn fetch_options(&self) -> webctl_web::transport::FetchOptions {
        webctl_web::transport::FetchOptions {
            timeout: std::time::Duration::from_millis(self.config.timeout_ms),
            proxy: self.config.proxy.clone(),
            browser_context: self.browser_context.clone(),
            ..webctl_web::transport::FetchOptions::default()
        }
    }
}

/// 从 JSON 文本解析浏览器上下文（单独函数，便于测试）。
///
/// 解析失败会向 stderr 打印原因并返回 `None` —— **不静默**（用户需要知道上下文没生效）。
pub fn parse_browser_context(text: &str) -> Option<webctl_web::browser_context::BrowserContext> {
    match serde_json::from_str::<webctl_web::browser_context::BrowserContext>(text) {
        Ok(context) if context.is_empty() => None,
        Ok(context) => Some(context),
        Err(error) => {
            eprintln!("webctl: 浏览器上下文解析失败，已忽略：{error}");
            None
        }
    }
}

/// 环境变量 `WEBCTL_BROWSER_CONTEXT` 指向 JSON 文件；未设置即 `None`。
pub fn browser_context_from_env() -> Option<webctl_web::browser_context::BrowserContext> {
    let path = std::env::var("WEBCTL_BROWSER_CONTEXT").ok()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_browser_context(&text),
        Err(error) => {
            eprintln!("webctl: 读取浏览器上下文文件失败（{path}）：{error}");
            None
        }
    }
}

/// 便捷：一个临时目录（测试用）。
pub fn temp_state_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("webctl-mcp-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("temp state dir");
    dir
}

#[cfg(test)]
mod browser_context_tests {
    use super::*;

    #[test]
    fn parses_full_context() {
        let json =
            r#"{"user_agent":"UA/1","cookies":[{"domain":".e.test","name":"sid","value":"V"}]}"#;
        let ctx = parse_browser_context(json).expect("应解析成功");
        assert_eq!(ctx.user_agent.as_deref(), Some("UA/1"));
        assert_eq!(ctx.cookies.len(), 1);
    }

    #[test]
    fn empty_context_is_none() {
        assert!(parse_browser_context("{}").is_none(), "空上下文等于不注入");
    }

    #[test]
    fn invalid_json_is_none_not_panic() {
        assert!(parse_browser_context("not json").is_none());
    }
}
