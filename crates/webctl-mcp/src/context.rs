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
}

impl McpContext {
    /// 用配置与 owner 构造。
    pub fn new(config: Config, owner: Option<String>) -> Self {
        Self {
            config,
            binding: SessionBinding::new(owner),
        }
    }

    /// 未绑定的上下文（测试或独立运行）。
    pub fn unbound(config: Config) -> Self {
        Self {
            config,
            binding: SessionBinding::unbound(),
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
            ..webctl_web::transport::FetchOptions::default()
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
