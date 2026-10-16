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
//!
//! 覆盖边界（如实记录，未覆盖的不得宣称“多进程已全测”）：
//! - 已覆盖：同一进程内两个独立 open（两把句柄 / 两个线程）真实互斥；持锁者
//!   drop / 持有线程结束后可再次获取；磁盘上遗留的锁文件本身不构成锁。
//! - **未覆盖**：真正 kill 一个独立持锁进程后由内核自动释放锁。flock 语义为
//!   最后一个 fd 关闭即释放，本模块只用同进程内的 drop / 线程结束模拟“句柄
//!   消失”（见 holder_disappears_releases_lock），零子进程约束下无法真正 kill。
//! - **未覆盖**：NFS 及一般网络文件系统的 flock 语义（本地文件系统可能与
//!   网络文件系统不同），无任何断言。

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

    /// 锁文件残留本身不构成锁：flock 只有真正持有 fd 才互斥。
    ///
    /// 注意：本用例只是往磁盘写了一个锁文件，**从未真正持锁**，因此不能用来
    /// 宣称覆盖“持锁进程被杀”；后者见 holder_disappears_releases_lock。
    #[test]
    fn stale_lock_file_does_not_block() {
        let dir = testutil::temp_dir("lock-stale");
        let path = dir.join("session.lock");
        std::fs::write(&path, b"stale pid 12345").expect("stale file");
        let lock = FileLock::acquire(&path, Duration::from_millis(200));
        assert!(lock.is_ok(), "残留锁文件不得阻塞新写者");
        testutil::cleanup(&dir);
    }

    /// 持锁者消失（句柄关闭 / 持有线程结束）后锁被释放，不残留死锁。
    ///
    /// 用同进程内的 drop / 线程结束模拟“持锁进程退出时内核关闭 fd 并释放
    /// flock”。零子进程约束下**没有**真正 kill 独立进程，见模块文档的覆盖边界。
    #[test]
    fn holder_disappears_releases_lock_without_deadlock() {
        let dir = testutil::temp_dir("lock-holder-gone");
        let path = dir.join("session.lock");

        // 持有线程结束 → 线程内的 FileLock drop → fd 关闭 → 锁释放。
        let holder_path = path.clone();
        let holder = std::thread::spawn(move || {
            let lock =
                FileLock::acquire(&holder_path, Duration::from_millis(200)).expect("holder lock");
            assert!(lock.path().ends_with("session.lock"));
            // 不显式 release：让 Drop 走内核释放路径，模拟进程消失。
        });
        holder.join().expect("holder thread");

        assert!(
            FileLock::try_acquire(&path).expect("try").is_some(),
            "持锁者消失后锁必须可再次获取，不得残留死锁"
        );
        testutil::cleanup(&dir);
    }
}
