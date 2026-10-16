//! state dir 解析：CLI 与 MCP 的单一入口（PLAN v0.4 §3.3，PR 05）。
//!
//! 优先级（高 → 低）：显式参数（CLI --state-dir 语义） > 环境变量
//! WEBCTL_STATE_DIR > 用户主目录下的 .webctl（HOME，Windows 回退 USERPROFILE） >
//! 当前目录下的 .webctl。
//!
//! 空值语义：显式参数为空（长度 0）、WEBCTL_STATE_DIR=""、HOME/USERPROFILE 为空
//! 时都视为“本层未给出”，继续向下回退；最终兜底是 `./.webctl`，**任何情况下
//! 都不会返回空路径**。config::default_state_dir() 只是本函数的薄委托，全仓
//! 不存在第二份 state dir 拼装。
//!
//! 解析逻辑做成纯函数 resolve_state_dir_from，环境值由调用方传入：
//! edition 2024 下 set_var 属 unsafe，而本 workspace 禁止 unsafe，
//! 因此测试用注入值覆盖优先级，而不是改动进程环境。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// 环境变量名：显式覆盖 state dir。
pub const STATE_DIR_ENV: &str = "WEBCTL_STATE_DIR";
/// 默认目录名（HOME 或当前目录下）。
pub const STATE_DIR_NAME: &str = ".webctl";

/// 解析 state dir（读取真实进程环境）。
pub fn resolve_state_dir(explicit: Option<&Path>) -> PathBuf {
    let env_dir = std::env::var_os(STATE_DIR_ENV);
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    resolve_state_dir_from(explicit, env_dir.as_deref(), home.as_deref())
}

/// 注入环境值的纯函数版本；空值视为未设置。
pub fn resolve_state_dir_from(
    explicit: Option<&Path>,
    env_dir: Option<&OsStr>,
    home: Option<&OsStr>,
) -> PathBuf {
    if let Some(path) = explicit.filter(|path| !path.as_os_str().is_empty()) {
        return path.to_path_buf();
    }
    if let Some(dir) = env_dir.filter(|value| !value.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(home) = home.filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join(STATE_DIR_NAME);
    }
    PathBuf::from(STATE_DIR_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn os(value: &str) -> OsString {
        OsString::from(value)
    }

    #[test]
    fn explicit_wins_over_env_and_home() {
        let explicit = Path::new("/tmp/explicit-state");
        assert_eq!(
            resolve_state_dir_from(Some(explicit), Some(&os("/tmp/env")), Some(&os("/home/u"))),
            explicit.to_path_buf()
        );
    }

    #[test]
    fn env_wins_over_home() {
        assert_eq!(
            resolve_state_dir_from(None, Some(&os("/tmp/env")), Some(&os("/home/u"))),
            PathBuf::from("/tmp/env")
        );
    }

    #[test]
    fn home_is_used_when_env_is_absent() {
        assert_eq!(
            resolve_state_dir_from(None, None, Some(&os("/home/u"))),
            PathBuf::from("/home/u").join(STATE_DIR_NAME)
        );
    }

    #[test]
    fn empty_values_fall_through_to_cwd() {
        assert_eq!(
            resolve_state_dir_from(
                Some(Path::new("")),
                Some(OsStr::new("")),
                Some(OsStr::new(""))
            ),
            PathBuf::from(STATE_DIR_NAME)
        );
        assert_eq!(
            resolve_state_dir_from(None, None, None),
            PathBuf::from(STATE_DIR_NAME)
        );
    }

    /// 四层优先级逐层断言：显式 > 环境变量 > HOME/.webctl > ./.webctl。
    #[test]
    fn four_level_priority_is_total() {
        let explicit = Path::new("/tmp/explicit-state");
        let env_dir = OsStr::new("/tmp/env-state");
        let home = OsStr::new("/home/u");
        assert_eq!(
            resolve_state_dir_from(Some(explicit), Some(env_dir), Some(home)),
            PathBuf::from("/tmp/explicit-state"),
            "第 1 层：显式参数压过环境变量与 HOME"
        );
        assert_eq!(
            resolve_state_dir_from(None, Some(env_dir), Some(home)),
            PathBuf::from("/tmp/env-state"),
            "第 2 层：环境变量压过 HOME"
        );
        assert_eq!(
            resolve_state_dir_from(None, None, Some(home)),
            PathBuf::from("/home/u").join(STATE_DIR_NAME),
            "第 3 层：HOME/.webctl"
        );
        assert_eq!(
            resolve_state_dir_from(None, None, None),
            PathBuf::from(STATE_DIR_NAME),
            "第 4 层：./.webctl 兜底"
        );
    }

    /// 空串语义：WEBCTL_STATE_DIR="" 与“缺省”同解，绝不返回空路径。
    #[test]
    fn empty_env_is_equivalent_to_absent_and_never_empty() {
        let home = OsStr::new("/home/u");
        let empty = OsStr::new("");
        assert_eq!(
            resolve_state_dir_from(None, Some(empty), Some(home)),
            resolve_state_dir_from(None, None, Some(home)),
            "空的环境变量必须与未设置同解"
        );
        assert_eq!(
            resolve_state_dir_from(None, Some(empty), Some(home)),
            PathBuf::from(home).join(STATE_DIR_NAME)
        );
        // 显式参数为空也视为缺省。
        assert_eq!(
            resolve_state_dir_from(Some(Path::new("")), Some(empty), Some(home)),
            PathBuf::from(home).join(STATE_DIR_NAME)
        );
        // 全部为空时兜底 ./.webctl，而不是空路径。
        let fallback = resolve_state_dir_from(None, Some(empty), Some(empty));
        assert_eq!(fallback, PathBuf::from(STATE_DIR_NAME));
        assert!(!fallback.as_os_str().is_empty(), "绝不能返回空路径");
    }

    /// 单一入口必须与既有 Config::default 的解析一致（同一环境快照下），且非空。
    #[test]
    fn real_resolution_matches_config_default() {
        let resolved = resolve_state_dir(None);
        assert_eq!(resolved, crate::config::default_state_dir());
        assert_eq!(crate::config::Config::default().state_dir, resolved);
        assert!(
            !resolved.as_os_str().is_empty(),
            "真实解析也绝不能返回空路径"
        );
    }
}
