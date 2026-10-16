//! 下载门禁与路径约束（SEC-11 / SEC-14；TESTING §2.14）。
//!
//! 两层：
//! 1. **类型白名单 + 尺寸上限**（`DownloadPolicy`）：默认严格，缺 Content-Type
//!    或非白名单类型一律拒绝；声明长度或实际长度越过上限一律拒绝（不静默截断）。
//! 2. **路径 containment**（复用 `webctl_core::security::paths::SafeRoot`）：
//!    远端给出的文件名只允许单段名字，解析后仍须落在下载根目录内，
//!    越界 symlink 与 `../` 一律拒绝。
//!
//! 落盘使用 `write_secret_file`（0600）——下载内容可能含凭据，权限与状态文件同级。

use std::path::{Path, PathBuf};

use webctl_core::ErrorCode;
use webctl_core::security::paths::{ArgumentError, SafeRoot, write_secret_file};

pub use webctl_core::security::paths::{ensure_http_scheme, is_http_scheme};

/// 默认下载大小上限（32 MiB）。
pub const DEFAULT_MAX_DOWNLOAD_BYTES: usize = 32 * 1024 * 1024;

/// 允许落盘的 Content-Type 白名单（essence，不含参数）。
pub const ALLOWED_DOWNLOAD_TYPES: [&str; 12] = [
    "text/html",
    "application/xhtml+xml",
    "text/plain",
    "text/markdown",
    "text/csv",
    "application/json",
    "application/xml",
    "text/xml",
    "application/pdf",
    "image/png",
    "image/jpeg",
    "image/webp",
];

/// 下载门禁错误（映射到封闭错误码）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    /// 路径越界 / 非法。
    Unsafe(ArgumentError),
    /// 服务端没有给出 Content-Type（默认严格：拒绝）。
    MissingContentType,
    /// Content-Type 不在白名单。
    TypeNotAllowed(String),
    /// 超过尺寸上限（不截断，明确拒绝）。
    TooLarge {
        /// 上限。
        limit: usize,
        /// 服务端声明的长度。
        declared: Option<u64>,
        /// 实际字节数。
        actual: usize,
    },
    /// 落盘失败。
    Io(String),
}

impl DownloadError {
    /// 映射到结果信封错误码。
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Unsafe(_) | Self::MissingContentType | Self::TypeNotAllowed(_) => {
                ErrorCode::Blocked
            }
            Self::TooLarge { .. } => ErrorCode::Limit,
            Self::Io(_) => ErrorCode::Internal,
        }
    }

    /// 人类可读说明（不含堆栈）。
    pub fn message(&self) -> String {
        match self {
            Self::Unsafe(error) => error.message(),
            Self::MissingContentType => {
                "download rejected: response has no Content-Type".to_owned()
            }
            Self::TypeNotAllowed(content_type) => {
                format!("download rejected: content type {content_type:?} is not in the allow-list")
            }
            Self::TooLarge {
                limit,
                declared,
                actual,
            } => format!(
                "download rejected: size limit {limit} bytes exceeded (declared={declared:?}, actual={actual})"
            ),
            Self::Io(message) => format!("download failed: {message}"),
        }
    }
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl std::error::Error for DownloadError {}

/// 下载策略：类型白名单 + 大小上限。
#[derive(Debug, Clone)]
pub struct DownloadPolicy {
    max_bytes: usize,
    allowed: Vec<String>,
}

impl Default for DownloadPolicy {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_DOWNLOAD_BYTES,
            allowed: ALLOWED_DOWNLOAD_TYPES
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
        }
    }
}

impl DownloadPolicy {
    /// 收紧大小上限。
    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// 大小上限。
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Content-Type 的 essence 是否在白名单内。
    pub fn is_allowed_type(&self, content_type: &str) -> bool {
        let essence = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        self.allowed.iter().any(|allowed| allowed == &essence)
    }

    /// 执行门禁：类型先于尺寸（类型不在白名单时直接拒绝）。
    pub fn check(
        &self,
        content_type: Option<&str>,
        declared_len: Option<u64>,
        actual_len: usize,
    ) -> Result<(), DownloadError> {
        let Some(content_type) = content_type else {
            return Err(DownloadError::MissingContentType);
        };
        if !self.is_allowed_type(content_type) {
            return Err(DownloadError::TypeNotAllowed(content_type.to_owned()));
        }
        let over_limit = declared_len.is_some_and(|declared| declared > self.max_bytes as u64)
            || actual_len > self.max_bytes;
        if over_limit {
            return Err(DownloadError::TooLarge {
                limit: self.max_bytes,
                declared: declared_len,
                actual: actual_len,
            });
        }
        Ok(())
    }
}

/// 通过门禁后把字节写入 `<root>/<file_name>`（0600）。
///
/// 调用顺序刻意是：先解析路径（containment），再查策略，最后落盘——
/// 任何一步失败都不会产生文件。
pub fn commit_download(
    root: &SafeRoot,
    file_name: &str,
    content_type: Option<&str>,
    declared_len: Option<u64>,
    bytes: &[u8],
    policy: &DownloadPolicy,
) -> Result<PathBuf, DownloadError> {
    let path = root
        .resolve_file_name(file_name)
        .map_err(DownloadError::Unsafe)?;
    policy.check(content_type, declared_len, bytes.len())?;
    std::fs::create_dir_all(root.root())
        .map_err(|error| DownloadError::Io(format!("create {}: {error}", root.root().display())))?;
    let _ = webctl_core::state::restrict_dir(root.root());
    write_secret_file(&path, bytes)
        .map_err(|error| DownloadError::Io(format!("write {}: {error}", path.display())))?;
    Ok(path)
}

/// 校验目标目录：不存在则创建并收紧权限。
pub fn ensure_download_root(root: &Path) -> Result<SafeRoot, DownloadError> {
    std::fs::create_dir_all(root)
        .map_err(|error| DownloadError::Io(format!("create {}: {error}", root.display())))?;
    let _ = webctl_core::state::restrict_dir(root);
    Ok(SafeRoot::new(root.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "webctl-sec-download-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn whitelist_and_size_limit_are_enforced() {
        let policy = DownloadPolicy::default();
        assert!(
            policy
                .check(Some("text/html; charset=utf-8"), None, 10)
                .is_ok()
        );
        assert!(matches!(
            policy.check(Some("application/zip"), None, 10),
            Err(DownloadError::TypeNotAllowed(_))
        ));
        assert!(matches!(
            policy.check(None, None, 10),
            Err(DownloadError::MissingContentType)
        ));
        let tight = DownloadPolicy::default().with_max_bytes(4);
        assert!(matches!(
            tight.check(Some("text/plain"), Some(5), 5),
            Err(DownloadError::TooLarge { .. })
        ));
        assert!(matches!(
            tight.check(Some("text/plain"), None, 5),
            Err(DownloadError::TooLarge { .. })
        ));
        assert_eq!(
            tight
                .check(Some("text/plain"), Some(9), 9)
                .unwrap_err()
                .code(),
            ErrorCode::Limit
        );
    }

    #[test]
    fn commit_download_writes_inside_root_only() {
        let root_dir = temp_dir("commit");
        let root = SafeRoot::new(root_dir.clone());
        let policy = DownloadPolicy::default();
        let path = commit_download(
            &root,
            "page.html",
            Some("text/html"),
            Some(4),
            b"<h1>",
            &policy,
        )
        .expect("allowed download");
        assert!(path.starts_with(&root_dir));
        assert_eq!(std::fs::read(&path).expect("read"), b"<h1>");
        #[cfg(unix)]
        assert_eq!(
            webctl_core::state::file_mode(&path),
            Some(webctl_core::state::FILE_MODE)
        );
        std::fs::remove_dir_all(&root_dir).ok();
    }

    #[test]
    fn commit_download_rejects_traversal_and_symlink() {
        let root_dir = temp_dir("escape");
        let outside = temp_dir("outside");
        let root = SafeRoot::new(root_dir.clone());
        let policy = DownloadPolicy::default();
        assert!(matches!(
            commit_download(
                &root,
                "../escape.html",
                Some("text/html"),
                None,
                b"x",
                &policy
            ),
            Err(DownloadError::Unsafe(_))
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(outside.join("victim"), root_dir.join("link.html")).expect("symlink");
            assert!(matches!(
                commit_download(&root, "link.html", Some("text/html"), None, b"x", &policy),
                Err(DownloadError::Unsafe(_))
            ));
            assert!(!outside.join("victim").exists(), "越界目标不得被创建/覆盖");
        }
        std::fs::remove_dir_all(&root_dir).ok();
        std::fs::remove_dir_all(&outside).ok();
    }
}
