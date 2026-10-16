//! 会话归属（MCP-07 / PLAN v0.4 §3.6）。
//!
//! DSH 契约：MCP server 绑定**确切 live Agent + Session**，执行期必须拒绝跨 Session 调用。
//! 本模块只做纯逻辑判定，不碰网络与磁盘，便于单元测试钉住边界。

use webctl_web::session::validate_session_name;

/// 未显式命名时使用的会话名。
pub const DEFAULT_SESSION: &str = "default";

/// 会话绑定：owner 为空表示未绑定（独立运行/测试），此时接受任意合法会话名。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionBinding {
    owner: Option<String>,
}

/// 归属判定失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnershipError {
    /// 请求的会话与绑定会话不一致。
    CrossSession {
        /// 绑定的会话名。
        owner: String,
        /// 调用方请求的会话名。
        requested: String,
    },
    /// 会话名本身非法（长度/字符集）。
    InvalidName(String),
}

impl std::fmt::Display for OwnershipError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CrossSession { owner, requested } => write!(
                formatter,
                "会话归属拒绝：本 MCP server 绑定会话 {owner:?}，调用方请求 {requested:?}（跨 Session 调用不被允许）"
            ),
            Self::InvalidName(message) => write!(formatter, "非法会话名：{message}"),
        }
    }
}

impl SessionBinding {
    /// 未绑定：接受任意合法会话名。
    pub fn unbound() -> Self {
        Self { owner: None }
    }

    /// 绑定到一个会话；空串等价于未绑定。
    pub fn new(owner: Option<String>) -> Self {
        let owner = owner.filter(|name| !name.trim().is_empty());
        Self { owner }
    }

    /// 绑定会话名（若有）。
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    /// 解析本次调用实际使用的会话名，并做归属/合法性校验。
    pub fn resolve(&self, requested: Option<&str>) -> Result<String, OwnershipError> {
        if let Some(owner) = &self.owner {
            return match requested {
                None => Ok(owner.clone()),
                Some(name) if name == owner => Ok(owner.clone()),
                Some(name) => Err(OwnershipError::CrossSession {
                    owner: owner.clone(),
                    requested: name.to_owned(),
                }),
            };
        }
        let name = requested.unwrap_or(DEFAULT_SESSION).to_owned();
        validate_session_name(&name)
            .map_err(|error| OwnershipError::InvalidName(error.message()))?;
        Ok(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_session_accepts_only_itself() {
        let binding = SessionBinding::new(Some("agent-7".to_owned()));
        assert_eq!(binding.resolve(None).as_deref(), Ok("agent-7"));
        assert_eq!(binding.resolve(Some("agent-7")).as_deref(), Ok("agent-7"));
        assert_eq!(
            binding.resolve(Some("agent-8")),
            Err(OwnershipError::CrossSession {
                owner: "agent-7".to_owned(),
                requested: "agent-8".to_owned(),
            })
        );
    }

    #[test]
    fn cross_session_error_is_readable_and_mentions_both() {
        let error = SessionBinding::new(Some("alpha".to_owned()))
            .resolve(Some("beta"))
            .expect_err("must reject");
        let message = error.to_string();
        assert!(
            message.contains("alpha") && message.contains("beta"),
            "{message}"
        );
        assert!(message.contains("归属拒绝"), "{message}");
    }

    #[test]
    fn unbound_defaults_to_default_and_validates() {
        let binding = SessionBinding::unbound();
        assert_eq!(binding.resolve(None).as_deref(), Ok(DEFAULT_SESSION));
        assert!(binding.resolve(Some("ok-name")).is_ok());
        assert!(matches!(
            binding.resolve(Some("../escape")),
            Err(OwnershipError::InvalidName(_))
        ));
    }
}
