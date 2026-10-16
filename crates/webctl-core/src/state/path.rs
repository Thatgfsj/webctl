//! 路径安全：任何落盘路径必须落在 state dir 内（PLAN v0.4 §3.3 / STATE-07）。
//!
//! 两层校验：
//! 1. 词法层：候选路径必须逐段以根目录开头，且不得含 .. / 绝对路径前缀；
//! 2. 真实层：对候选路径与根目录 canonicalize（不存在的尾部取最近存在的祖先
//!    再拼接），拒绝越界的符号链接；坏掉的 symlink 直接拒绝。

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::error::ErrorCode;

/// 路径越界或非法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// 词法上已经逃出根目录（..、绝对路径等）。
    Escapes { root: PathBuf, candidate: PathBuf },
    /// 符号链接解析后逃出根目录（含坏链接）。
    SymlinkEscape { root: PathBuf, candidate: PathBuf },
    /// 单段名字非法（空、分隔符、. / ..、控制字符、冒号）。
    InvalidSegment { segment: String },
    /// 文件系统错误。
    Io(String),
}

impl PathError {
    /// 映射到结果信封错误码：越界/非法按“明确拒绝”归 BLOCKED。
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Io(_) => ErrorCode::Internal,
            Self::Escapes { .. } | Self::SymlinkEscape { .. } | Self::InvalidSegment { .. } => {
                ErrorCode::Blocked
            }
        }
    }

    /// 人类可读说明。
    pub fn message(&self) -> String {
        match self {
            Self::Escapes { root, candidate } => format!(
                "path escapes state dir: {} is not under {}",
                candidate.display(),
                root.display()
            ),
            Self::SymlinkEscape { root, candidate } => format!(
                "symlink escapes state dir: {} resolves outside {}",
                candidate.display(),
                root.display()
            ),
            Self::InvalidSegment { segment } => {
                format!("invalid path segment {segment:?}")
            }
            Self::Io(message) => format!("path check failed: {message}"),
        }
    }
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for PathError {}

/// 校验单段名字是否可安全拼进目录。
pub fn validate_segment(segment: &str) -> Result<(), PathError> {
    let invalid = || PathError::InvalidSegment {
        segment: segment.to_owned(),
    };
    if segment.is_empty() || segment == "." || segment == ".." {
        return Err(invalid());
    }
    if segment.contains('/') || segment.contains('\\') || segment.contains(':') {
        return Err(invalid());
    }
    if segment.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(())
}

/// 词法判断 candidate 是否落在 root 内（不触碰文件系统）。
pub fn is_within_lexically(root: &Path, candidate: &Path) -> bool {
    let mut candidate_components = candidate.components();
    for root_component in root.components() {
        match candidate_components.next() {
            Some(component) if component == root_component => {}
            _ => return false,
        }
    }
    !candidate_components.any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

/// 组合 root/name，并保证结果落在 root 内（词法 + 符号链接）。
pub fn safe_join(root: &Path, name: &str) -> Result<PathBuf, PathError> {
    validate_segment(name)?;
    let candidate = root.join(name);
    ensure_contained(root, &candidate)?;
    Ok(candidate)
}

/// 确认 candidate（允许尚不存在）解析后仍落在 root 内。
pub fn ensure_contained(root: &Path, candidate: &Path) -> Result<(), PathError> {
    if !is_within_lexically(root, candidate) {
        return Err(PathError::Escapes {
            root: root.to_path_buf(),
            candidate: candidate.to_path_buf(),
        });
    }
    // 坏掉的 symlink：canonicalize 会失败，这里显式归为越界而不是 IO 错误。
    if let Ok(metadata) = fs::symlink_metadata(candidate)
        && metadata.file_type().is_symlink()
        && candidate.canonicalize().is_err()
    {
        return Err(PathError::SymlinkEscape {
            root: root.to_path_buf(),
            candidate: candidate.to_path_buf(),
        });
    }
    let root_resolved =
        canonicalize_nearest(root).map_err(|error| PathError::Io(error.to_string()))?;
    let candidate_resolved =
        canonicalize_nearest(candidate).map_err(|error| PathError::Io(error.to_string()))?;
    if !candidate_resolved.starts_with(&root_resolved) {
        return Err(PathError::SymlinkEscape {
            root: root.to_path_buf(),
            candidate: candidate.to_path_buf(),
        });
    }
    Ok(())
}

/// canonicalize 一个可能不存在的路径：取最近存在的祖先，再拼回缺失的尾部。
fn canonicalize_nearest(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut current = absolute;
    let mut missing: Vec<OsString> = Vec::new();
    loop {
        match fs::symlink_metadata(&current) {
            Ok(_) => {
                let mut resolved = current.canonicalize()?;
                for part in missing.iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound {
                    return Err(error);
                }
                let parent = current.parent().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "no existing ancestor")
                })?;
                match current.file_name() {
                    Some(name) => {
                        missing.push(name.to_os_string());
                        current = parent.to_path_buf();
                    }
                    None => return Err(error),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testutil;

    #[test]
    fn rejects_separators_dots_and_control() {
        for bad in ["", ".", "..", "a/b", "a\\b", "a:b", "a\nb", "a\0b"] {
            assert!(validate_segment(bad).is_err(), "{bad:?} 必须被拒绝");
        }
        for good in ["default", "user-1", "a.b_c", "sess.json"] {
            assert!(validate_segment(good).is_ok(), "{good:?} 应被接受");
        }
    }

    #[test]
    fn lexical_check_rejects_parent_and_absolute() {
        let root = Path::new("/state");
        assert!(is_within_lexically(
            root,
            Path::new("/state/sessions/a.json")
        ));
        assert!(!is_within_lexically(
            root,
            Path::new("/state/../etc/passwd")
        ));
        assert!(!is_within_lexically(root, Path::new("/etc/passwd")));
        assert!(!is_within_lexically(
            Path::new("state"),
            Path::new("/state")
        ));
    }

    #[test]
    fn safe_join_allows_missing_file_but_not_traversal() {
        let dir = testutil::temp_dir("path-safe-join");
        let root = dir.join("sessions");
        let path = safe_join(&root, "default.json").expect("missing file under missing dir");
        assert!(path.starts_with(&root));
        assert!(matches!(
            safe_join(&root, "../escape.json"),
            Err(PathError::InvalidSegment { .. })
        ));
        testutil::cleanup(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        use std::os::unix::fs::symlink;
        let dir = testutil::temp_dir("path-symlink");
        let root = dir.join("sessions");
        fs::create_dir_all(&root).expect("session dir");
        let outside = dir.join("outside");
        fs::create_dir_all(&outside).expect("outside dir");
        let target = outside.join("victim.json");
        fs::write(&target, b"{}").expect("target");
        symlink(&target, root.join("link.json")).expect("symlink");
        assert!(matches!(
            ensure_contained(&root, &root.join("link.json")),
            Err(PathError::SymlinkEscape { .. })
        ));
        // 坏链接同样拒绝。
        symlink(outside.join("missing.json"), root.join("broken.json")).expect("broken symlink");
        assert!(matches!(
            ensure_contained(&root, &root.join("broken.json")),
            Err(PathError::SymlinkEscape { .. })
        ));
        testutil::cleanup(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn internal_symlink_is_allowed() {
        use std::os::unix::fs::symlink;
        let dir = testutil::temp_dir("path-symlink-ok");
        let root = dir.join("sessions");
        fs::create_dir_all(&root).expect("session dir");
        fs::write(root.join("real.json"), b"{}").expect("real");
        symlink(root.join("real.json"), root.join("alias.json")).expect("symlink");
        assert!(ensure_contained(&root, &root.join("alias.json")).is_ok());
        testutil::cleanup(&dir);
    }
}
