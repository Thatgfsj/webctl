//! 会话落盘存储：state dir + advisory 文件锁 + TTL + 权限 0600（PR 05 / #34）。
//!
//! 序列化、导入导出与名字校验复用 session::state 的 SessionState 与
//! validate_session_name；本模块只负责“怎么安全地落盘/读回”。
//!
//! 路径安全：containment root 恒为 **state root**，而不是 <state>/sessions 子目录。
//! save / load / remove / list / purge_expired / path_for / lock_path 每个入口
//! 都先经 ensure_contained(state_root, ...) 把关；<state>/sessions 本身是指向
//! state root 之外的 symlink 时全部返回可读的 UnsafePath（映射 BLOCKED），
//! 绝不读写或删除 state dir 之外的文件。
//!
//! 并发模型：每个会话一个 <sessions>/.<name>.lock，使用 webctl_core::state 的
//! flock 语义 advisory 锁。锁属于 open file description，因此同一进程内的两个
//! 独立 open（两个线程、两把句柄）也真实互斥；跨进程由同一内核锁语义保证。
//! 本实现与测试都不创建子进程（零子进程硬约束）。
//!
//! 互斥语义的覆盖边界（如实记录，未覆盖的不得宣称“已全测”）：
//! - **未覆盖**：持锁进程被信号杀死后由内核自动释放锁。内核只在最后一个 fd
//!   关闭时释放 flock；本模块只用同一进程内的 drop / 持有线程结束来模拟
//!   “句柄消失”（见 state::lock 的 holder_disappears 用例），**没有**真正
//!   kill 一个独立进程验证。既有的“残留锁文件”用例只是写了个锁文件、从未
//!   真正持锁，不能据此宣称覆盖了“持锁者被杀”。
//! - **未覆盖**：NFS 及一般网络文件系统的 flock 语义。本模块只在本地文件系统
//!   上验证，网络文件系统上的锁语义可能不同，无任何断言。
//!
//! 原子写：先写同目录临时文件并 fsync，再 rename 覆盖目标；临时文件与目标文件
//! 权限均为 0600。写入被中断只会留下临时文件，绝不会被当成会话读取。

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use webctl_core::state as core_state;
use webctl_core::state::{DEFAULT_LOCK_WAIT, FileLock, LockError, PathError};

use super::state::{
    DEFAULT_TTL, SESSION_FILE_VERSION, SessionError, SessionState, validate_session_name,
};

/// 会话存储：所有文件都落在 root/sessions 下。
#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
    ttl: Duration,
    lock_wait: Duration,
}

impl SessionStore {
    /// 以默认 TTL 创建存储。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_ttl(root, DEFAULT_TTL)
    }

    /// 指定 TTL。
    pub fn with_ttl(root: impl Into<PathBuf>, ttl: Duration) -> Self {
        Self {
            root: root.into(),
            ttl,
            lock_wait: DEFAULT_LOCK_WAIT,
        }
    }

    /// 用核心层的单一入口解析 state dir（CLI 与 MCP 共用同一条路径）。
    pub fn from_state_dir(explicit: Option<&Path>) -> Self {
        Self::new(core_state::resolve_state_dir(explicit))
    }

    /// 调整锁等待上限。
    pub fn with_lock_wait(mut self, wait: Duration) -> Self {
        self.lock_wait = wait;
        self
    }

    /// state 根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 会话目录（root/sessions）。
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    /// 默认 TTL。
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// 锁等待上限。
    pub fn lock_wait(&self) -> Duration {
        self.lock_wait
    }

    /// 会话文件路径；非法名字或越界 symlink 直接报错。
    ///
    /// 校验以 **state root** 为 containment root（不是 sessions 子目录），
    /// 因此 `<state>/sessions` 本身是越界 symlink 时这里就返回 UnsafePath，
    /// 不会把 state dir 之外的路径交给上层。
    pub fn path_for(&self, name: &str) -> Result<PathBuf, SessionError> {
        validate_session_name(name)?;
        self.path_in_sessions(&format!("{name}.json"))
    }

    /// 会话锁文件路径（同样以 state root 为 containment root）。
    pub fn lock_path(&self, name: &str) -> Result<PathBuf, SessionError> {
        validate_session_name(name)?;
        self.path_in_sessions(&format!(".{name}.lock"))
    }

    /// 已校验的 sessions 目录：canonicalize 后必须仍落在 state root 内。
    ///
    /// 这是所有落盘入口的唯一把关点；`sessions_dir()` 只返回未校验的原始路径，
    /// 内部使用一律先经过这里。
    fn checked_sessions_dir(&self) -> Result<PathBuf, SessionError> {
        let sessions = self.sessions_dir();
        core_state::ensure_contained(&self.root, &sessions).map_err(path_error)?;
        Ok(sessions)
    }

    /// sessions 下的候选路径：先确认 sessions 目录不越界，再确认叶节点不越界。
    fn path_in_sessions(&self, file_name: &str) -> Result<PathBuf, SessionError> {
        let candidate = self.checked_sessions_dir()?.join(file_name);
        core_state::ensure_contained(&self.root, &candidate).map_err(path_error)?;
        Ok(candidate)
    }

    /// 新建一个（尚未落盘的）会话状态。
    pub fn new_state(&self, name: &str) -> Result<SessionState, SessionError> {
        validate_session_name(name)?;
        Ok(SessionState::new(name, self.ttl))
    }

    /// 原子写入会话文件（锁内 + 临时文件 + rename），权限 0600。
    pub fn save(&self, state: &SessionState) -> Result<PathBuf, SessionError> {
        let path = self.path_for(&state.name)?;
        self.ensure_dirs()?;
        let _lock = self.acquire(&state.name)?;
        self.cleanup_stale_temps(&state.name);
        let mut state = state.clone();
        state.version = SESSION_FILE_VERSION;
        state.updated_at = now_secs();
        let json = state.export_json()?;
        let tmp = self.temp_path(&state.name)?;
        if let Err(error) = write_temp(&tmp, json.as_bytes()) {
            let _ = fs::remove_file(&tmp);
            return Err(error);
        }
        if let Err(error) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(SessionError::Io(format!(
                "rename into {}: {error}",
                path.display()
            )));
        }
        let _ = core_state::restrict_file(&path);
        Ok(path)
    }

    /// 读取会话；不存在/过期/损坏各自给出可读错误，绝不静默重建。
    pub fn load(&self, name: &str) -> Result<SessionState, SessionError> {
        let path = self.path_for(name)?;
        let _lock = self.acquire(name)?;
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(SessionError::NotFound(name.to_owned()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                return Err(SessionError::Corrupt(format!(
                    "invalid session file encoding: {error}"
                )));
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
        let _lock = self.acquire(name)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(SessionError::Io(format!(
                "remove {}: {error}",
                path.display()
            ))),
        }
    }

    /// 当前目录下的会话名（排序；不含临时文件与锁文件）。
    pub fn list(&self) -> Result<Vec<String>, SessionError> {
        let dir = self.checked_sessions_dir()?;
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(SessionError::Io(format!("read {}: {error}", dir.display())));
            }
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| SessionError::Io(error.to_string()))?;
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let Some(stem) = file_name.strip_suffix(".json") else {
                continue;
            };
            if stem.starts_with('.') || validate_session_name(stem).is_err() {
                continue;
            }
            if entry.path().is_file() {
                names.push(stem.to_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    /// 回收已过期会话，返回回收数量；不触碰未过期会话与损坏文件。
    pub fn purge_expired(&self) -> Result<usize, SessionError> {
        let mut purged = 0;
        for name in self.list()? {
            let path = self.path_for(&name)?;
            let still_expired = || {
                fs::read_to_string(&path)
                    .ok()
                    .and_then(|raw| SessionState::import_json(&raw).ok())
                    .is_some_and(|state| state.is_expired(now_secs()))
            };
            if !still_expired() {
                continue;
            }
            match self.acquire(&name) {
                Ok(_lock) => {
                    // 锁内复查，避免删掉刚被写者刷新的会话。
                    if still_expired() && fs::remove_file(&path).is_ok() {
                        purged += 1;
                    }
                }
                Err(SessionError::LockTimeout(_)) => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(purged)
    }

    fn ensure_dirs(&self) -> Result<(), SessionError> {
        let sessions = self.checked_sessions_dir()?;
        fs::create_dir_all(&sessions)
            .map_err(|error| SessionError::Io(format!("create {}: {error}", sessions.display())))?;
        let _ = core_state::restrict_dir(&self.root);
        let _ = core_state::restrict_dir(&sessions);
        Ok(())
    }

    fn acquire(&self, name: &str) -> Result<FileLock, SessionError> {
        let path = self.lock_path(name)?;
        FileLock::acquire(&path, self.lock_wait).map_err(|error| match error {
            LockError::Timeout { .. } => SessionError::LockTimeout(name.to_owned()),
            LockError::Io { message, .. } => {
                SessionError::Io(format!("lock {}: {message}", path.display()))
            }
        })
    }

    fn temp_path(&self, name: &str) -> Result<PathBuf, SessionError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        Ok(self
            .checked_sessions_dir()?
            .join(format!(".{name}.tmp{nanos:032x}")))
    }

    fn cleanup_stale_temps(&self, name: &str) {
        let prefix = format!(".{name}.tmp");
        let Ok(sessions) = self.checked_sessions_dir() else {
            return;
        };
        let Ok(entries) = fs::read_dir(&sessions) else {
            return;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            if file_name.to_string_lossy().starts_with(&prefix) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn path_error(error: PathError) -> SessionError {
    SessionError::UnsafePath(error.message())
}

fn write_temp(path: &Path, bytes: &[u8]) -> Result<(), SessionError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| SessionError::Io(format!("create {}: {error}", path.display())))?;
    let _ = core_state::restrict_file(path);
    file.write_all(bytes)
        .map_err(|error| SessionError::Io(format!("write {}: {error}", path.display())))?;
    file.sync_all()
        .map_err(|error| SessionError::Io(format!("sync {}: {error}", path.display())))?;
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
