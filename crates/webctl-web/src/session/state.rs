//! 会话标识、持久化与导入导出（PLAN v0.4 §2「会话」域，PR 04；STATE-* 归属 PR 05）。
//!
//! 本模块提供协议级会话引擎所需的最小状态面：
//! - 会话名校验（拒绝路径穿越、绝对路径、控制字符），保证文件只落在 state dir 内；
//! - cookie 快照的导入/导出（结构层面，凭据脱敏在 PR 12 兜底）；
//! - TTL：过期会话返回可读错误，不静默重建；
//! - 原子写 + 简易锁文件（完整的并发/锁超时语义在 PR 05）。
//!
//! 硬约束：零子进程；不调用任何进程创建 API。

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
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
    /// 非法会话名（尤其路径穿越）按“明确拒绝”归入 BLOCKED；
    /// 不存在/过期/损坏归入 SESSION_LOST，方便调用方区分“未登录”与“被拒绝”。
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidName(_) => ErrorCode::Blocked,
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
            Self::NotFound(name) => format!("session {name:?} not found"),
            Self::Expired(name) => format!("session {name:?} has expired"),
            Self::Corrupt(name) => {
                format!("session {name:?} file is corrupt; refusing to silently recreate it")
            }
            Self::Io(message) => format!("session storage error: {message}"),
            Self::LockTimeout(name) => {
                format!("timed out waiting for session {name:?} lock held by another process")
            }
        }
    }
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

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

/// 会话存储：所有文件都落在 root/sessions 下。
#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
    ttl: Duration,
}

impl SessionStore {
    /// 以默认 TTL 创建存储。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            ttl: DEFAULT_TTL,
        }
    }

    /// 指定 TTL。
    pub fn with_ttl(root: impl Into<PathBuf>, ttl: Duration) -> Self {
        Self {
            root: root.into(),
            ttl,
        }
    }

    /// 默认 TTL。
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// 会话目录（root/sessions）。
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    /// 会话文件的绝对路径；非法名字直接报错。
    pub fn path_for(&self, name: &str) -> Result<PathBuf, SessionError> {
        validate_session_name(name)?;
        Ok(self.sessions_dir().join(format!("{name}.json")))
    }

    /// 新建一个（尚未落盘的）会话状态。
    pub fn new_state(&self, name: &str) -> Result<SessionState, SessionError> {
        validate_session_name(name)?;
        Ok(SessionState::new(name, self.ttl))
    }

    /// 原子写入会话文件（临时文件 + rename），并加简易文件锁。
    pub fn save(&self, state: &SessionState) -> Result<PathBuf, SessionError> {
        let path = self.path_for(&state.name)?;
        self.ensure_dirs()?;
        let _lock = self.acquire_lock(&state.name)?;
        let mut state = state.clone();
        state.version = SESSION_FILE_VERSION;
        state.updated_at = now_secs();
        let json = state.export_json()?;
        let tmp = self.temp_path(&state.name);
        {
            let mut file = fs::File::create(&tmp)
                .map_err(|error| SessionError::Io(format!("create {}: {error}", tmp.display())))?;
            file.write_all(json.as_bytes())
                .map_err(|error| SessionError::Io(format!("write {}: {error}", tmp.display())))?;
            file.sync_all()
                .map_err(|error| SessionError::Io(format!("sync {}: {error}", tmp.display())))?;
        }
        restrict_file(&tmp, 0o600);
        fs::rename(&tmp, &path).map_err(|error| {
            SessionError::Io(format!("rename into {}: {error}", path.display()))
        })?;
        restrict_file(&path, 0o600);
        Ok(path)
    }

    /// 读取会话；不存在/过期/损坏各自给出可读错误，绝不静默重建。
    pub fn load(&self, name: &str) -> Result<SessionState, SessionError> {
        let path = self.path_for(name)?;
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(SessionError::NotFound(name.to_owned()));
            }
            Err(error) => {
                return Err(SessionError::Io(format!(
                    "read {}: {error}",
                    path.display()
                )));
            }
        };
        let state = SessionState::import_json(&raw)?;
        if state.is_expired(now_secs()) {
            return Err(SessionError::Expired(name.to_owned()));
        }
        Ok(state)
    }

    /// 会话是否存在且未过期。
    pub fn exists(&self, name: &str) -> bool {
        self.load(name).is_ok()
    }

    /// 删除会话文件（只删自己的会话）。
    pub fn remove(&self, name: &str) -> Result<bool, SessionError> {
        let path = self.path_for(name)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(SessionError::Io(format!(
                "remove {}: {error}",
                path.display()
            ))),
        }
    }

    fn ensure_dirs(&self) -> Result<(), SessionError> {
        let dir = self.sessions_dir();
        fs::create_dir_all(&dir)
            .map_err(|error| SessionError::Io(format!("create {}: {error}", dir.display())))?;
        restrict_file(&dir, 0o700);
        Ok(())
    }

    fn temp_path(&self, name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        self.sessions_dir().join(format!(".{name}.tmp{nanos:032x}"))
    }

    fn lock_path(&self, name: &str) -> PathBuf {
        self.sessions_dir().join(format!(".{name}.lock"))
    }

    fn acquire_lock(&self, name: &str) -> Result<LockGuard, SessionError> {
        let path = self.lock_path(name);
        for _ in 0..100 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let _ = write!(file, "locked");
                    restrict_file(&path, 0o600);
                    return Ok(LockGuard { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(SessionError::Io(format!(
                        "lock {}: {error}",
                        path.display()
                    )));
                }
            }
        }
        Err(SessionError::LockTimeout(name.to_owned()))
    }
}

struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(unix)]
fn restrict_file(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path, _mode: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("webctl-session-{label}-{nanos}"));
        fs::create_dir_all(&dir).expect("tempdir");
        dir
    }

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
    fn path_for_stays_inside_sessions_dir() {
        let dir = tempdir("path");
        let store = SessionStore::new(&dir);
        let path = store.path_for("default").expect("path");
        assert!(path.starts_with(store.sessions_dir()));
        assert!(store.path_for("../escape").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn roundtrip_import_export_and_ttl() {
        let dir = tempdir("roundtrip");
        let store = SessionStore::with_ttl(&dir, Duration::from_secs(60));
        let mut state = store.new_state("default").expect("state");
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
        store.save(&state).expect("save");
        let loaded = store.load("default").expect("load");
        assert_eq!(loaded.cookies.len(), 1);

        // 过期会话必须报错而不是返回旧数据。
        let expired = SessionState {
            expires_at: Some(1),
            ..loaded.clone()
        };
        store.save(&expired).expect("save expired");
        assert_eq!(
            store.load("default"),
            Err(SessionError::Expired("default".to_owned()))
        );

        // 损坏文件不静默重建。
        fs::write(store.path_for("default").expect("path"), "{not json").expect("write");
        assert!(matches!(
            store.load("default"),
            Err(SessionError::Corrupt(_))
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_session_is_not_found() {
        let dir = tempdir("missing");
        let store = SessionStore::new(&dir);
        assert_eq!(
            store.load("default"),
            Err(SessionError::NotFound("default".to_owned()))
        );
        assert_eq!(
            SessionError::NotFound("x".into()).code(),
            ErrorCode::SessionLost
        );
        assert_eq!(
            SessionError::InvalidName("x".into()).code(),
            ErrorCode::Blocked
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
