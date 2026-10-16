//! 会话标识与序列化（PLAN v0.4 §2「会话」域；PR 04 建立，PR 05 拆分）。
//!
//! 本模块保留协议级会话引擎需要的最小状态面：
//! - 会话名校验（拒绝路径穿越、绝对路径、控制字符）；
//! - cookie 快照的导入/导出（结构层面；凭据脱敏在 PR 12 兜底）；
//! - TTL 判定（过期返回可读错误，不静默重建）。
//!
//! 落盘（state dir + advisory 文件锁 + 原子写 + 权限）已拆到 session::store
//! （PR 05 / #34），避免同一份逻辑散在两处。
//!
//! 硬约束：零子进程；不调用任何进程创建 API。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use webctl_core::ErrorCode;

use crate::transport::StoredCookie;

/// 会话文件格式版本。
pub const SESSION_FILE_VERSION: u32 = 1;
/// 默认会话 TTL（7 天）。
pub const DEFAULT_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// 会话名长度上限。
pub const MAX_SESSION_NAME_LEN: usize = 128;

/// 会话层错误（映射到封闭错误码）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// 会话名非法（含路径穿越）。
    InvalidName(String),
    /// 落盘路径不安全（越界 symlink / 词法逃逸）。
    UnsafePath(String),
    /// 会话不存在。
    NotFound(String),
    /// 会话已过期。
    Expired(String),
    /// 会话文件损坏或版本不兼容。
    Corrupt(String),
    /// 文件系统错误。
    Io(String),
    /// 获取会话锁超时。
    LockTimeout(String),
}

impl SessionError {
    /// 映射到结果信封错误码。
    ///
    /// 非法会话名/越界路径按“明确拒绝”归入 BLOCKED；
    /// 不存在/过期/损坏/锁超时归入 SESSION_LOST，方便调用方区分“未登录”与“被拒绝”。
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidName(_) | Self::UnsafePath(_) => ErrorCode::Blocked,
            Self::NotFound(_) | Self::Expired(_) | Self::Corrupt(_) | Self::LockTimeout(_) => {
                ErrorCode::SessionLost
            }
            Self::Io(_) => ErrorCode::Internal,
        }
    }

    /// 人类可读说明。
    pub fn message(&self) -> String {
        match self {
            Self::InvalidName(name) => format!(
                "invalid session name {name:?}: must be non-empty [A-Za-z0-9._-], no separators"
            ),
            Self::UnsafePath(message) => format!("unsafe session path: {message}"),
            Self::NotFound(name) => format!("session {name:?} not found"),
            Self::Expired(name) => format!("session {name:?} has expired"),
            Self::Corrupt(name) => {
                format!("session {name:?} file is corrupt; refusing to silently recreate it")
            }
            Self::Io(message) => format!("session storage error: {message}"),
            Self::LockTimeout(name) => format!(
                "timed out waiting for session {name:?} lock held by another writer; retry later"
            ),
        }
    }
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for SessionError {}

/// 校验会话名是否可安全用作文件名。
pub fn validate_session_name(name: &str) -> Result<(), SessionError> {
    let invalid = |reason: &str| Err(SessionError::InvalidName(format!("{name} ({reason})")));
    if name.is_empty() {
        return invalid("empty");
    }
    if name.len() > MAX_SESSION_NAME_LEN {
        return invalid("too long");
    }
    if name == "." || name == ".." || name.contains("..") {
        return invalid("path traversal");
    }
    if name.starts_with('.') {
        return invalid("hidden name");
    }
    for ch in name.chars() {
        let allowed = ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.');
        if !allowed {
            return invalid("illegal character");
        }
    }
    Ok(())
}

/// 可持久化的会话状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    /// 文件格式版本。
    pub version: u32,
    /// 会话名。
    pub name: String,
    /// 创建时间（unix 秒）。
    pub created_at: u64,
    /// 最后更新时间（unix 秒）。
    pub updated_at: u64,
    /// 过期时间（unix 秒）；None 表示不过期。
    pub expires_at: Option<u64>,
    /// cookie 快照（可空）。
    pub cookies: Vec<StoredCookie>,
    /// 已访问的来源（导入导出的结构信息）。
    pub origins: Vec<String>,
}

impl SessionState {
    /// 新建一个会话状态，按给定 TTL 计算过期时间。
    pub fn new(name: impl Into<String>, ttl: Duration) -> Self {
        let now = now_secs();
        let expires_at = ttl.as_secs().checked_add(now);
        Self {
            version: SESSION_FILE_VERSION,
            name: name.into(),
            created_at: now,
            updated_at: now,
            expires_at,
            cookies: Vec::new(),
            origins: Vec::new(),
        }
    }

    /// 是否已过期。
    pub fn is_expired(&self, now: u64) -> bool {
        self.expires_at.is_some_and(|expires| expires <= now)
    }

    /// 记录一个来源。
    pub fn note_origin(&mut self, origin: &str) {
        if !self.origins.iter().any(|seen| seen == origin) {
            self.origins.push(origin.to_owned());
        }
    }

    /// 导出为 JSON（结构层面；脱敏在 PR 12）。
    pub fn export_json(&self) -> Result<String, SessionError> {
        serde_json::to_string_pretty(self)
            .map_err(|error| SessionError::Io(format!("serialize session: {error}")))
    }

    /// 从 JSON 导入，做版本与名字校验。
    pub fn import_json(raw: &str) -> Result<Self, SessionError> {
        let state: Self = serde_json::from_str(raw)
            .map_err(|error| SessionError::Corrupt(format!("invalid session json: {error}")))?;
        if state.version != SESSION_FILE_VERSION {
            return Err(SessionError::Corrupt(format!(
                "unsupported session file version {}",
                state.version
            )));
        }
        validate_session_name(&state.name)?;
        Ok(state)
    }
}

/// 当前 unix 秒。
pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_separators() {
        for bad in [
            "../evil", "a/b", "a\\b", "", ".", "..", ".hidden", "a b", "a:b", "a\0b",
        ] {
            assert!(validate_session_name(bad).is_err(), "{bad:?} 必须被拒绝");
        }
        for good in ["default", "user-1", "a.b_c"] {
            assert!(validate_session_name(good).is_ok(), "{good:?} 应被接受");
        }
    }

    #[test]
    fn roundtrip_import_export() {
        let mut state = SessionState::new("default", Duration::from_secs(60));
        state.note_origin("http://127.0.0.1:1");
        state.cookies.push(StoredCookie {
            name: "sid".to_owned(),
            value: "abc".to_owned(),
            domain: "127.0.0.1".to_owned(),
            host_only: true,
            path: "/".to_owned(),
            secure: false,
            http_only: true,
            expires_at: None,
        });
        let json = state.export_json().expect("export");
        let imported = SessionState::import_json(&json).expect("import");
        assert_eq!(imported, state);
    }

    #[test]
    fn ttl_and_corruption_are_readable_errors() {
        let expired = SessionState {
            expires_at: Some(1),
            ..SessionState::new("default", Duration::from_secs(60))
        };
        assert!(expired.is_expired(now_secs()));
        assert_eq!(
            SessionError::Expired("default".into()).code(),
            ErrorCode::SessionLost
        );
        assert_eq!(
            SessionError::Corrupt("default".into()).code(),
            ErrorCode::SessionLost
        );
        assert!(matches!(
            SessionState::import_json("{not json"),
            Err(SessionError::Corrupt(_))
        ));
        assert_eq!(
            SessionError::NotFound("x".into()).code(),
            ErrorCode::SessionLost
        );
        assert_eq!(
            SessionError::InvalidName("x".into()).code(),
            ErrorCode::Blocked
        );
        assert_eq!(
            SessionError::UnsafePath("x".into()).code(),
            ErrorCode::Blocked
        );
    }

    #[test]
    fn unknown_version_is_corrupt() {
        let mut state = SessionState::new("default", Duration::from_secs(60));
        state.version = 99;
        let json = state.export_json().expect("export");
        assert!(matches!(
            SessionState::import_json(&json),
            Err(SessionError::Corrupt(_))
        ));
    }
}
