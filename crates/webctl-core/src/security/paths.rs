//! 文件参数 containment 与 URL scheme 白名单（SEC-11 / SEC-15）。
//!
//! 复用 PR 05 的 `crate::state::path` 原语：词法层拒绝 `..`，真实层
//! canonicalize 后必须仍落在给定 root 内（越界 symlink 一律拒绝）。
//! 本模块只做“把外部输入约束到 root”，不决定 root 是什么（调用方给）。
//!
//! 默认严格：**绝对路径直接拒绝**（SEC-11 明确要求），相对路径按 root 解析。

use std::path::{Path, PathBuf};

use crate::error::ErrorCode;
pub use crate::state::path::{
    PathError, ensure_contained, is_within_lexically, safe_join, validate_segment,
};

/// 文件参数被拒。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentError {
    /// 绝对路径：文件参数必须是相对 root 的相对路径。
    Absolute {
        /// 原始参数。
        argument: PathBuf,
    },
    /// 越界或非法（`..`、越界 symlink、坏链接等）。
    Unsafe(PathError),
}

impl ArgumentError {
    /// 映射到封闭错误码：明确拒绝统一归 BLOCKED。
    pub fn code(&self) -> ErrorCode {
        ErrorCode::Blocked
    }

    /// 人类可读说明（不含堆栈）。
    pub fn message(&self) -> String {
        match self {
            Self::Absolute { argument } => format!(
                "absolute path {argument:?} rejected: file arguments must stay relative to the allowed root"
            ),
            Self::Unsafe(error) => error.message(),
        }
    }
}

impl std::fmt::Display for ArgumentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl std::error::Error for ArgumentError {}

/// 把相对文件参数解析到 root 下；绝对路径与越界一律拒绝。
pub fn resolve_file_argument(root: &Path, requested: &Path) -> Result<PathBuf, ArgumentError> {
    if requested.is_absolute() {
        return Err(ArgumentError::Absolute {
            argument: requested.to_path_buf(),
        });
    }
    let candidate = root.join(requested);
    ensure_contained(root, &candidate).map_err(ArgumentError::Unsafe)?;
    Ok(candidate)
}

/// 一个受信任的根目录；所有落在其中的文件参数都应由它解析。
#[derive(Debug, Clone)]
pub struct SafeRoot {
    root: PathBuf,
}

impl SafeRoot {
    /// 以 root 建立门禁（不要求 root 已存在；缺失尾部按最近存在的祖先校验）。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 解析相对参数；绝对路径与越界被拒。
    pub fn resolve(&self, requested: impl AsRef<Path>) -> Result<PathBuf, ArgumentError> {
        resolve_file_argument(&self.root, requested.as_ref())
    }

    /// 解析远端给出的单段文件名（不含分隔符）；随后仍做 containment 校验。
    pub fn resolve_file_name(&self, file_name: &str) -> Result<PathBuf, ArgumentError> {
        validate_segment(file_name).map_err(ArgumentError::Unsafe)?;
        self.resolve(Path::new(file_name))
    }
}

/// 以 0600 创建/覆盖一个敏感文件。
///
/// 权限在 **open 时**就设好（Unix `OpenOptionsExt::mode`），并在写入后再次收紧，
/// 因此即使目标文件原先权限更宽，也不会出现“先明文后收紧”的窗口。
pub fn write_secret_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    let _ = crate::state::perms::restrict_file(path);
    Ok(())
}

/// URL scheme 不合法（SEC-15）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemeError {
    /// 没有 `scheme://` 结构。
    Missing,
    /// scheme 存在但不是 http/https。
    Disallowed(String),
}

impl SchemeError {
    /// 非法协议按 BAD_URL 处理（与 transport 的既有语义一致）。
    pub fn code(&self) -> ErrorCode {
        ErrorCode::BadUrl
    }

    /// 人类可读说明。
    pub fn message(&self) -> String {
        match self {
            Self::Missing => "invalid url: missing scheme://".to_owned(),
            Self::Disallowed(scheme) => {
                format!("unsupported scheme {scheme:?}: only http/https are allowed")
            }
        }
    }
}

impl std::fmt::Display for SchemeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl std::error::Error for SchemeError {}

/// 抽取小写 scheme（RFC 3986 字符集）；无 scheme 返回 None。
pub fn scheme_of(raw: &str) -> Option<String> {
    let index = raw.find("://")?;
    let scheme = &raw[..index];
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
    {
        return None;
    }
    Some(scheme.to_ascii_lowercase())
}

/// 是否 http(s) URL。
pub fn is_http_scheme(raw: &str) -> bool {
    matches!(scheme_of(raw).as_deref(), Some("http") | Some("https"))
}

/// 只放行 http(s)，其余（含 `file://`、`data:`、`ftp://`）一律拒绝。
pub fn ensure_http_scheme(raw: &str) -> Result<(), SchemeError> {
    match scheme_of(raw) {
        Some(scheme) if scheme == "http" || scheme == "https" => Ok(()),
        Some(scheme) => Err(SchemeError::Disallowed(scheme)),
        None => Err(SchemeError::Missing),
    }
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
            "webctl-sec-path-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn rejects_traversal_and_absolute_arguments() {
        let root = temp_dir("reject");
        assert!(matches!(
            resolve_file_argument(&root, Path::new("../escape.json")),
            Err(ArgumentError::Unsafe(_))
        ));
        assert!(matches!(
            resolve_file_argument(&root, Path::new("/etc/passwd")),
            Err(ArgumentError::Absolute { .. })
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn allows_relative_argument_under_root() {
        let root = temp_dir("allow");
        let resolved = resolve_file_argument(&root, Path::new("sub/out.json")).expect("allowed");
        assert!(resolved.starts_with(&root));
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let root = temp_dir("symlink");
        let outside = temp_dir("outside");
        std::fs::write(outside.join("victim"), b"x").expect("victim");
        symlink(outside.join("victim"), root.join("link")).expect("symlink");
        assert!(matches!(
            resolve_file_argument(&root, Path::new("link")),
            Err(ArgumentError::Unsafe(_))
        ));
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[cfg(unix)]
    #[test]
    fn secret_file_is_mode_600_even_if_previously_wider() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir("secret-file");
        let path = root.join("cookies.json");
        std::fs::write(&path, b"{}").expect("seed");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        write_secret_file(&path, b"{\"cookie\":\"SECRET\"}").expect("write");
        assert_eq!(
            std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scheme_whitelist_rejects_file_and_data() {
        assert!(ensure_http_scheme("http://127.0.0.1/x").is_ok());
        assert!(ensure_http_scheme("HTTPS://example.test/x").is_ok());
        for bad in [
            "file:///etc/passwd",
            "data:text/plain,hi",
            "ftp://host/x",
            "not a url",
        ] {
            assert!(ensure_http_scheme(bad).is_err(), "{bad} 必须被拒绝");
        }
        assert_eq!(
            ensure_http_scheme("file:///etc/passwd").unwrap_err().code(),
            ErrorCode::BadUrl
        );
    }
}
