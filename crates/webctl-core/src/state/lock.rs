//! advisory 文件锁（flock 语义，PR 05）。
//!
//! 选型：fslock 0.2.1。理由：
//! - 公开 API 安全（unsafe 全在依赖内部），无构建脚本、无 C 工具链；
//! - Unix 上直接调用 flock(2)（见 fslock 源码 unix.rs），锁属于 open file
//!   description：同一进程内两个独立打开也会真实互斥，足以验证并发语义；
//! - 进程退出/被杀时内核自动释放锁，不会留下“锁文件残留即死锁”的坑；
//! - 已在 Cargo.lock（btls-sys 的构建依赖），离线可解析。
//!
//! 备选与差异：fs2 同样基于 flock，但 fslock 的 per-handle 语义有明确文档、
//! 且 try_lock 直接返回布尔值。rustix/libc 需要在本 crate 内写 unsafe 调用，
//! 会被 workspace 的 unsafe_code = forbid 卡住，故不采用。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::ErrorCode;
use crate::state::perms;

/// 默认等待上限。
pub const DEFAULT_LOCK_WAIT: Duration = Duration::from_secs(5);
/// 轮询间隔。
pub const DEFAULT_LOCK_POLL: Duration = Duration::from_millis(10);

/// 文件锁错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// 在等待上限内没有拿到锁（不静默放弃）。
    Timeout { path: PathBuf, waited_ms: u64 },
    /// 打开或加锁失败。
    Io { path: PathBuf, message: String },
}

impl LockError {
    /// 映射到结果信封错误码：拿不到会话锁视为会话状态不可用。
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Timeout { .. } => ErrorCode::SessionLost,
            Self::Io { .. } => ErrorCode::Internal,
        }
    }

    /// 人类可读说明。
    pub fn message(&self) -> String {
        match self {
            Self::Timeout { path, waited_ms } => format!(
                "timed out after {waited_ms}ms waiting for lock {} (held by another writer)",
                path.display()
            ),
            Self::Io { path, message } => {
                format!("lock {} failed: {message}", path.display())
            }
        }
    }
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for LockError {}

/// 持有的文件锁；Drop（或进程死亡）即释放。
#[derive(Debug)]
pub struct FileLock {
    file: fslock::LockFile,
    path: PathBuf,
}

impl FileLock {
    /// 非阻塞尝试；锁被占用时返回 Ok(None)。
    pub fn try_acquire(path: &Path) -> Result<Option<Self>, LockError> {
        let io = |error: fslock::Error| LockError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        };
        let mut file = fslock::LockFile::open(path).map_err(io)?;
        // 收紧锁文件权限；失败不阻断加锁本身（非 Unix 为 no-op）。
        let _ = perms::restrict_file(path);
        match file.try_lock() {
            Ok(true) => Ok(Some(Self {
                file,
                path: path.to_path_buf(),
            })),
            Ok(false) => Ok(None),
            Err(error) => Err(io(error)),
        }
    }

    /// 在 wait 内轮询获取；超时给出可读错误。
    pub fn acquire(path: &Path, wait: Duration) -> Result<Self, LockError> {
        let started = Instant::now();
        loop {
            if let Some(lock) = Self::try_acquire(path)? {
                return Ok(lock);
            }
            let waited = started.elapsed();
            if waited >= wait {
                return Err(LockError::Timeout {
                    path: path.to_path_buf(),
                    waited_ms: waited.as_millis() as u64,
                });
            }
            std::thread::sleep(DEFAULT_LOCK_POLL.min(wait));
        }
    }

    /// 锁保护的路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 显式释放。
    pub fn release(mut self) {
        let _ = self.file.unlock();
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        if self.file.owns_lock() {
            let _ = self.file.unlock();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testutil;

    /// 两个独立打开的文件句柄（模拟两个进程）真实互斥。
    #[test]
    fn independent_handles_are_mutually_exclusive() {
        let dir = testutil::temp_dir("lock-exclusive");
        let path = dir.join("session.lock");
        let held = FileLock::acquire(&path, Duration::from_millis(200)).expect("first lock");
        assert!(FileLock::try_acquire(&path).expect("try").is_none());
        assert!(matches!(
            FileLock::acquire(&path, Duration::from_millis(60)),
            Err(LockError::Timeout { .. })
        ));
        held.release();
        assert!(FileLock::acquire(&path, Duration::from_millis(200)).is_ok());
        testutil::cleanup(&dir);
    }

    /// 锁文件残留（持锁进程被杀）不造成死锁：flock 已由内核释放。
    #[test]
    fn stale_lock_file_does_not_block() {
        let dir = testutil::temp_dir("lock-stale");
        let path = dir.join("session.lock");
        std::fs::write(&path, b"stale pid 12345").expect("stale file");
        let lock = FileLock::acquire(&path, Duration::from_millis(200));
        assert!(lock.is_ok(), "残留锁文件不得阻塞新写者");
        testutil::cleanup(&dir);
    }
}
