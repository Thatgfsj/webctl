//! 文件权限：state 目录 0700、会话/凭据文件 0600（PLAN v0.4 §3.3 / STATE-01）。
//!
//! 非 Unix 平台没有 POSIX mode：收紧是显式 no-op，查询返回 None，
//! 不会伪装成“已收紧”。

use std::fs;
use std::io;
use std::path::Path;

/// state 目录权限位。
pub const DIR_MODE: u32 = 0o700;
/// 会话/凭据文件权限位。
pub const FILE_MODE: u32 = 0o600;

/// 把目录权限收紧为 0700。
pub fn restrict_dir(path: &Path) -> io::Result<()> {
    set_mode(path, DIR_MODE)
}

/// 把文件权限收紧为 0600。
pub fn restrict_file(path: &Path) -> io::Result<()> {
    set_mode(path, FILE_MODE)
}

/// 读取权限位（含 setuid/setgid/sticky）。非 Unix 返回 None。
pub fn mode_of(path: &Path) -> io::Result<Option<u32>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(Some(fs::metadata(path)?.permissions().mode() & 0o7777))
    }
    #[cfg(not(unix))]
    {
        let _ = fs::metadata(path)?;
        Ok(None)
    }
}

/// 目录权限位；读取失败或非 Unix 返回 None。
pub fn dir_mode(path: &Path) -> Option<u32> {
    mode_of(path).ok().flatten()
}

/// 文件权限位；读取失败或非 Unix 返回 None。
pub fn file_mode(path: &Path) -> Option<u32> {
    mode_of(path).ok().flatten()
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testutil;

    #[test]
    fn file_mode_is_restricted_or_explicitly_absent() {
        let dir = testutil::temp_dir("perms-file");
        let file = dir.join("session.json");
        fs::write(&file, b"{}").expect("write");
        restrict_file(&file).expect("restrict");
        #[cfg(unix)]
        assert_eq!(file_mode(&file), Some(FILE_MODE));
        #[cfg(not(unix))]
        assert_eq!(file_mode(&file), None, "非 Unix 必须显式 None，而不是假通过");
        testutil::cleanup(&dir);
    }

    #[test]
    fn dir_mode_is_restricted_or_explicitly_absent() {
        let dir = testutil::temp_dir("perms-dir");
        let nested = dir.join("nested");
        fs::create_dir_all(&nested).expect("create");
        restrict_dir(&nested).expect("restrict");
        #[cfg(unix)]
        assert_eq!(dir_mode(&nested), Some(DIR_MODE));
        #[cfg(not(unix))]
        assert_eq!(dir_mode(&nested), None);
        testutil::cleanup(&dir);
    }

    #[test]
    fn mode_of_missing_path_is_error() {
        let dir = testutil::temp_dir("perms-missing");
        assert!(mode_of(&dir.join("nope")).is_err());
        testutil::cleanup(&dir);
    }
}
