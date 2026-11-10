//! Playwright `storage_state` 导出（0.2.3 M6 / #226）。
//!
//! 目标：把 webctl 会话里的 cookie 与（由调用方提供的）origin/localStorage
//! 导出为 **Playwright 兼容**的 storage_state JSON，让 agent 在 webctl 登录后
//! 可以把登录态交给 Playwright 复用。
//!
//! # 结构（字段名与 Playwright 规范逐字对齐）
//!
//! ```json
//! {
//!   "cookies": [
//!     {
//!       "name": "sid",
//!       "value": "...",
//!       "domain": "example.test",
//!       "path": "/",
//!       "expires": 1700000000,
//!       "httpOnly": true,
//!       "secure": true,
//!       "sameSite": "Lax"
//!     }
//!   ],
//!   "origins": [
//!     {
//!       "origin": "https://example.test",
//!       "localStorage": [{ "name": "token", "value": "..." }]
//!     }
//!   ]
//! }
//! ```
//!
//! **不做**：新增工具面、调用 Playwright、读写浏览器 profile、访问网络。
//! 本模块是纯 Rust 的数据转换 + 受控落盘。
//!
//! # 安全边界（必须与文档/PR 的威胁模型一致）
//!
//! - **最小域**：只导出 `target_domain` 命中的 cookie 与 origin；域名按标签
//!   比较（cookie 域等于目标域，或是目标的子域），不是字符串 `ends_with`。
//! - **文件权限**：Unix 下用 `OpenOptionsExt::mode(0o600)` 创建，并对已存在
//!   文件再次 `set_permissions(0o600)`；非 Unix 明确报 `Unsupported`，绝不
//!   退化成 0644。
//! - **路径**：拒绝任何含 `..` 组件的路径；叶节点是 symlink/目录时拒绝。
//!   该检查是**写入前**的最佳努力，调用方仍必须把目标放在自己控制的目录。
//! - **日志**：本模块不使用 `tracing`/`log`；`Debug` 实现把 cookie 值与
//!   localStorage 值替换为平台无关常量 `***`，避免调试输出泄漏。
//! - **落盘内容必须原样**：不得对写入文件的 cookie 值做 "***" 脱敏，否则
//!   Playwright 无法复用登录态。脱敏只用于日志/诊断。
//!
//! # 已知不保真项（如实记录，不宣称完全等价）
//!
//! - webctl 的 `StoredCookie` 未持久化 `SameSite`，导出默认 `Lax`（浏览器
//!   对缺失属性的默认行为），调用方可用 [`StorageStateCookie::with_same_site`] 覆盖；
//! - Playwright 的 storage_state 没有 `hostOnly` 字段，host-only cookie 以裸域
//!   导出，与 Playwright 自身导出的形态一致；
//! - localStorage 不是 webctl 抓取/执行产生的，必须由调用方显式提供，本模块
//!   不猜测、不合成。

use std::fmt;
use std::fs;
use std::path::{Component, Path};

use serde::Serialize;
use webctl_core::ErrorCode;
use webctl_core::security::storage_state::LOG_REDACTED;

use crate::session::SessionState;
use crate::transport::StoredCookie;

/// 会话 cookie 在 Playwright storage_state 中的 `expires` 取值。
pub const SESSION_COOKIE_EXPIRES: i64 = -1;

/// 导出文件权限（Unix）。
pub const STORAGE_STATE_FILE_MODE: u32 = 0o600;

/// 与 Playwright storage_state JSON 对应的顶层结构。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StorageState {
    /// 命中的 cookie 列表。
    pub cookies: Vec<StorageStateCookie>,
    /// 命中的 origin 及其 localStorage。
    pub origins: Vec<StorageStateOrigin>,
}

/// 单个 cookie；serde 字段名严格对齐 Playwright（`httpOnly` / `sameSite`）。
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStateCookie {
    /// cookie 名。
    pub name: String,
    /// cookie 值（敏感；落盘时保留原值）。
    pub value: String,
    /// 作用域域名（小写、无前导点）。
    pub domain: String,
    /// 作用域路径。
    pub path: String,
    /// 过期时间（unix 秒）；`-1` 表示会话 cookie。
    pub expires: i64,
    /// 仅脚本不可见。
    pub http_only: bool,
    /// 仅 https 下发。
    pub secure: bool,
    /// SameSite 策略。
    pub same_site: SameSite,
}

/// Playwright 接受的 SameSite 字面量。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SameSite {
    /// `SameSite=Strict`。
    Strict,
    /// `SameSite=Lax`（webctl 未记录 SameSite 时的默认值）。
    #[default]
    Lax,
    /// `SameSite=None`。
    None,
}

impl StorageStateCookie {
    /// 从会话 cookie 快照转换；未记录的 SameSite 按浏览器默认取 `Lax`。
    pub fn from_stored(cookie: &StoredCookie) -> Self {
        Self {
            name: cookie.name.clone(),
            value: cookie.value.clone(),
            domain: cookie.domain.clone(),
            path: cookie.path.clone(),
            expires: cookie
                .expires_at
                .map(|secs| i64::try_from(secs).unwrap_or(i64::MAX))
                .unwrap_or(SESSION_COOKIE_EXPIRES),
            http_only: cookie.http_only,
            secure: cookie.secure,
            same_site: SameSite::default(),
        }
    }

    /// 覆盖 SameSite（当调用方从其它来源知道真实值时使用）。
    pub fn with_same_site(mut self, same_site: SameSite) -> Self {
        self.same_site = same_site;
        self
    }
}

/// 单个 origin 的 localStorage 条目。
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct LocalStorageEntry {
    /// 键。
    pub name: String,
    /// 值（敏感；落盘时保留原值）。
    pub value: String,
}

impl LocalStorageEntry {
    /// 构造条目。
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// 一个 origin 及其 localStorage。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStateOrigin {
    /// origin 字符串（例如 `https://example.test`）。
    pub origin: String,
    /// localStorage 条目；没有时为空数组。
    pub local_storage: Vec<LocalStorageEntry>,
}

impl StorageStateOrigin {
    /// 构造 origin；localStorage 默认为空。
    pub fn new(origin: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            local_storage: Vec::new(),
        }
    }

    /// 追加一条 localStorage。
    pub fn with_local_storage(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.local_storage.push(LocalStorageEntry::new(name, value));
        self
    }
}

// 手写 Debug：cookie / localStorage 的**值**永远不进调试输出。
impl fmt::Debug for StorageStateCookie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StorageStateCookie")
            .field("name", &self.name)
            .field("value", &LOG_REDACTED)
            .field("domain", &self.domain)
            .field("path", &self.path)
            .field("expires", &self.expires)
            .field("http_only", &self.http_only)
            .field("secure", &self.secure)
            .field("same_site", &self.same_site)
            .finish()
    }
}

impl fmt::Debug for LocalStorageEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalStorageEntry")
            .field("name", &self.name)
            .field("value", &LOG_REDACTED)
            .finish()
    }
}

impl StorageState {
    /// 序列化为 Playwright 可读的 JSON（未脱敏，供直接落盘）。
    pub fn to_json_pretty(&self) -> Result<String, StorageStateError> {
        serde_json::to_string_pretty(self)
            .map_err(|error| StorageStateError::Serialize(error.to_string()))
    }

    /// 按 0600 权限写入调用方指定路径。
    pub fn write_to_path(&self, path: &Path) -> Result<(), StorageStateError> {
        validate_output_path(path)?;
        let json = self.to_json_pretty()?;
        write_private(path, json.as_bytes())
    }
}

/// 只导出目标域命中的 cookie 与 origin。
///
/// `target_domain` 会先归一化（去空白/点、转小写、IDN 转 punycode）；
/// 非法或空目标域返回 [`StorageStateError::InvalidTargetDomain`]，而不是
/// "导出空结果"这种看起来成功的失败。
pub fn export_storage_state(
    target_domain: &str,
    cookies: &[StoredCookie],
    origins: &[StorageStateOrigin],
) -> Result<StorageState, StorageStateError> {
    let target = normalize_target_domain(target_domain)?;
    let cookies = cookies
        .iter()
        .filter(|cookie| domain_matches(&target, &cookie.domain))
        .map(StorageStateCookie::from_stored)
        .collect();
    let origins = origins
        .iter()
        .filter(|origin| origin_matches(&target, &origin.origin))
        .cloned()
        .collect();
    Ok(StorageState { cookies, origins })
}

/// 从 [`SessionState`] 导出：cookie 来自会话快照，origin 来自会话访问记录。
///
/// 会话记录里没有 localStorage，因此每个 origin 的 `localStorage` 都是空数组；
/// 需要 localStorage 的调用方请使用 [`export_storage_state`] 显式提供。
pub fn export_session_storage_state(
    state: &SessionState,
    target_domain: &str,
) -> Result<StorageState, StorageStateError> {
    let origins: Vec<StorageStateOrigin> =
        state.origins.iter().map(StorageStateOrigin::new).collect();
    export_storage_state(target_domain, &state.cookies, &origins)
}

/// 导出并按 0600 写入指定路径；路径在读取 cookie 前就校验。
pub fn export_to_path(
    target_domain: &str,
    cookies: &[StoredCookie],
    origins: &[StorageStateOrigin],
    path: &Path,
) -> Result<(), StorageStateError> {
    validate_output_path(path)?;
    let state = export_storage_state(target_domain, cookies, origins)?;
    state.write_to_path(path)
}

/// 域名是否命中：`host` 等于目标域，或是目标的子域（按标签比较）。
pub fn domain_matches(target_domain: &str, host: &str) -> bool {
    let target = normalize_domain(target_domain);
    let host = normalize_domain(host);
    if target.is_empty() || host.is_empty() {
        return false;
    }
    host == target
        || host
            .strip_suffix(&target)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

/// 输出路径安全校验：拒绝空路径、任何 `..` 组件、symlink 与目录。
///
/// 只做词法校验与写入前的 symlink 检查；不要求绝对路径（调用方可以显式给
/// 相对路径），也不创建父目录。`a..b.json` 这类文件名不会被误判。
pub fn validate_output_path(path: &Path) -> Result<(), StorageStateError> {
    if path.as_os_str().is_empty() {
        return Err(StorageStateError::UnsafePath("empty path".to_owned()));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(StorageStateError::UnsafePath(
            "path must not contain '..' components".to_owned(),
        ));
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(StorageStateError::UnsafePath(
                "refusing to write through a symlink".to_owned(),
            ));
        }
        Ok(metadata) if metadata.is_dir() => {
            return Err(StorageStateError::UnsafePath(
                "refusing to write to a directory".to_owned(),
            ));
        }
        _ => {}
    }
    Ok(())
}

/// storage_state 导出错误（映射到封闭错误码）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageStateError {
    /// 目标域为空或不是合法主机名。
    InvalidTargetDomain(String),
    /// 输出路径不安全（`..` / symlink / 目录）。
    UnsafePath(String),
    /// 当前平台无法保证 0600 权限。
    UnsupportedPlatform(String),
    /// JSON 序列化失败（不应出现）。
    Serialize(String),
    /// 文件系统错误。
    Io(String),
}

impl StorageStateError {
    /// 映射到结果信封错误码。
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidTargetDomain(_) | Self::UnsafePath(_) => ErrorCode::Blocked,
            Self::UnsupportedPlatform(_) => ErrorCode::Unsupported,
            Self::Serialize(_) | Self::Io(_) => ErrorCode::Internal,
        }
    }

    /// 人类可读说明（不含任何 cookie 值）。
    pub fn message(&self) -> String {
        match self {
            Self::InvalidTargetDomain(domain) => {
                format!("invalid target domain {domain:?}: expected a bare host name")
            }
            Self::UnsafePath(message) => format!("unsafe storage_state path: {message}"),
            Self::UnsupportedPlatform(message) => {
                format!("storage_state export is unsupported here: {message}")
            }
            Self::Serialize(message) => format!("serialize storage_state: {message}"),
            Self::Io(message) => format!("storage_state io: {message}"),
        }
    }
}

impl fmt::Display for StorageStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for StorageStateError {}

fn normalize_domain(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('.')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

fn normalize_target_domain(raw: &str) -> Result<String, StorageStateError> {
    let trimmed = raw.trim().trim_start_matches('.');
    if trimmed.is_empty() || trimmed.contains('/') || trimmed.chars().any(char::is_whitespace) {
        return Err(StorageStateError::InvalidTargetDomain(raw.to_owned()));
    }
    let host = url::Host::parse(trimmed)
        .map_err(|_| StorageStateError::InvalidTargetDomain(raw.to_owned()))?;
    let normalized = match host {
        url::Host::Domain(domain) => domain.trim_end_matches('.').to_ascii_lowercase(),
        url::Host::Ipv4(ip) => ip.to_string(),
        url::Host::Ipv6(ip) => ip.to_string(),
    };
    if normalized.is_empty() {
        return Err(StorageStateError::InvalidTargetDomain(raw.to_owned()));
    }
    Ok(normalized)
}

fn origin_matches(target_domain: &str, origin: &str) -> bool {
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let host = match url.host() {
        Some(url::Host::Domain(domain)) => domain.to_ascii_lowercase(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        None => return false,
    };
    domain_matches(target_domain, &host)
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), StorageStateError> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .mode(STORAGE_STATE_FILE_MODE)
        .open(path)
        .map_err(|error| StorageStateError::Io(format!("create {}: {error}", path.display())))?;
    // 已存在文件的 mode 不会被 O_CREAT 改变，这里显式收紧后再写入内容。
    file.set_permissions(fs::Permissions::from_mode(STORAGE_STATE_FILE_MODE))
        .map_err(|error| StorageStateError::Io(format!("chmod {}: {error}", path.display())))?;
    file.set_len(0)
        .map_err(|error| StorageStateError::Io(format!("truncate {}: {error}", path.display())))?;
    file.write_all(bytes)
        .map_err(|error| StorageStateError::Io(format!("write {}: {error}", path.display())))?;
    file.sync_all()
        .map_err(|error| StorageStateError::Io(format!("sync {}: {error}", path.display())))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(_path: &Path, _bytes: &[u8]) -> Result<(), StorageStateError> {
    Err(StorageStateError::UnsupportedPlatform(
        "0600 file permissions require Unix; refusing to write a credentials file with weaker permissions"
            .to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;

    fn stored(name: &str, value: &str, domain: &str) -> StoredCookie {
        StoredCookie {
            name: name.to_owned(),
            value: value.to_owned(),
            domain: domain.to_owned(),
            host_only: true,
            path: "/".to_owned(),
            secure: true,
            http_only: true,
            expires_at: Some(1_700_000_000),
        }
    }

    fn temp_path(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "webctl-storage-state-{tag}-{}-{nanos}.json",
            std::process::id()
        ))
    }

    fn sample_state() -> StorageState {
        let cookies = vec![stored("sid", "TARGET_SECRET", "example.test")];
        let origins = vec![
            StorageStateOrigin::new("https://example.test")
                .with_local_storage("token", "ORIGIN_SECRET"),
        ];
        export_storage_state("example.test", &cookies, &origins).expect("export")
    }

    #[test]
    fn json_fields_match_playwright_schema_exactly() {
        let state = sample_state();
        let value: serde_json::Value =
            serde_json::from_str(&state.to_json_pretty().expect("json")).expect("parse");
        assert_eq!(value.as_object().expect("object").len(), 2);

        let cookie = &value["cookies"][0];
        let object = cookie.as_object().expect("cookie object");
        assert_eq!(object.len(), 8, "unexpected fields: {cookie}");
        for key in [
            "name", "value", "domain", "path", "expires", "httpOnly", "secure", "sameSite",
        ] {
            assert!(object.contains_key(key), "missing {key}: {cookie}");
        }
        assert!(!object.contains_key("http_only"));
        assert!(!object.contains_key("same_site"));
        assert_eq!(cookie["name"], json!("sid"));
        assert_eq!(cookie["value"], json!("TARGET_SECRET"));
        assert_eq!(cookie["domain"], json!("example.test"));
        assert_eq!(cookie["path"], json!("/"));
        assert_eq!(cookie["expires"], json!(1_700_000_000_i64));
        assert_eq!(cookie["httpOnly"], json!(true));
        assert_eq!(cookie["secure"], json!(true));
        assert_eq!(cookie["sameSite"], json!("Lax"));

        let origin = &value["origins"][0];
        assert_eq!(origin["origin"], json!("https://example.test"));
        assert_eq!(origin["localStorage"][0]["name"], json!("token"));
        assert_eq!(origin["localStorage"][0]["value"], json!("ORIGIN_SECRET"));
        assert!(origin.get("local_storage").is_none());
    }

    #[test]
    fn session_cookie_uses_minus_one_expires() {
        let mut cookie = stored("sid", "SECRET", "example.test");
        cookie.expires_at = None;
        let state = export_storage_state("example.test", &[cookie], &[]).expect("export");
        assert_eq!(state.cookies[0].expires, SESSION_COOKIE_EXPIRES);
        assert_eq!(SESSION_COOKIE_EXPIRES, -1);
    }

    #[test]
    fn non_target_cookies_are_not_exported() {
        let cookies = vec![
            stored("sid", "TARGET_SECRET", "example.test"),
            stored("other", "OTHER_SECRET", "other.example"),
            stored("lookalike", "LOOKALIKE_SECRET", "notexample.test"),
            stored("suffix", "SUFFIX_SECRET", "example.test.evil"),
            stored("parent", "PARENT_SECRET", "test"),
        ];
        let state = export_storage_state("example.test", &cookies, &[]).expect("export");
        let serialized = state.to_json_pretty().expect("json");
        assert_eq!(state.cookies.len(), 1, "{serialized}");
        assert_eq!(state.cookies[0].name, "sid");
        assert!(serialized.contains("TARGET_SECRET"));
        for secret in [
            "OTHER_SECRET",
            "LOOKALIKE_SECRET",
            "SUFFIX_SECRET",
            "PARENT_SECRET",
        ] {
            assert!(
                !serialized.contains(secret),
                "{secret} leaked: {serialized}"
            );
        }
    }

    #[test]
    fn subdomain_cookie_is_exported() {
        let cookies = vec![stored("sid", "SUB_SECRET", ".login.example.test")];
        let state = export_storage_state("example.test", &cookies, &[]).expect("export");
        assert_eq!(state.cookies.len(), 1);
        assert_eq!(state.cookies[0].name, "sid");
        assert_eq!(state.cookies[0].domain, ".login.example.test");
    }

    #[test]
    fn domain_matching_is_label_aware() {
        assert!(domain_matches("example.test", "example.test"));
        assert!(domain_matches("example.test", ".sub.example.test"));
        assert!(domain_matches("Example.TEST", "sub.example.test."));
        assert!(!domain_matches("example.test", "notexample.test"));
        assert!(!domain_matches("example.test", "test"));
        assert!(!domain_matches("example.test", "example.test.evil"));
        assert!(!domain_matches("", "example.test"));
        assert!(!domain_matches("example.test", ""));
    }

    #[test]
    fn invalid_target_domain_is_rejected() {
        for bad in [
            "",
            "  ",
            "https://example.test",
            "example.test:443",
            "exa mple.test",
        ] {
            assert!(
                matches!(
                    export_storage_state(bad, &[], &[]),
                    Err(StorageStateError::InvalidTargetDomain(_))
                ),
                "{bad:?} must be rejected"
            );
        }
        assert!(export_storage_state("127.0.0.1", &[], &[]).is_ok());
    }

    #[test]
    fn origins_outside_target_are_not_exported() {
        let origins = vec![
            StorageStateOrigin::new("https://example.test")
                .with_local_storage("token", "TARGET_ORIGIN_SECRET"),
            StorageStateOrigin::new("https://login.example.test"),
            StorageStateOrigin::new("https://other.example")
                .with_local_storage("token", "OTHER_ORIGIN_SECRET"),
            StorageStateOrigin::new("ftp://example.test"),
            StorageStateOrigin::new("not a url"),
        ];
        let state = export_storage_state("example.test", &[], &origins).expect("export");
        let serialized = state.to_json_pretty().expect("json");
        assert_eq!(state.origins.len(), 2, "{serialized}");
        assert!(serialized.contains("TARGET_ORIGIN_SECRET"));
        assert!(!serialized.contains("OTHER_ORIGIN_SECRET"));
        assert!(!serialized.contains("ftp://"));
        assert!(!serialized.contains("not a url"));
    }

    #[test]
    fn debug_output_never_contains_credentials() {
        let state = sample_state();
        let rendered = format!("{state:?}");
        assert!(!rendered.contains("TARGET_SECRET"), "{rendered}");
        assert!(!rendered.contains("ORIGIN_SECRET"), "{rendered}");
        assert!(rendered.contains(LOG_REDACTED), "{rendered}");
    }

    #[test]
    fn parent_dir_paths_are_rejected() {
        for bad in ["../storage.json", "safe/../storage.json", "a/../../b.json"] {
            let error = validate_output_path(Path::new(bad)).expect_err("must reject");
            assert!(
                matches!(error, StorageStateError::UnsafePath(_)),
                "{bad:?}: {error}"
            );
            assert_eq!(error.code(), ErrorCode::Blocked);
        }
        assert!(validate_output_path(Path::new("storage..state.json")).is_ok());
        assert!(validate_output_path(Path::new("")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn exported_file_is_0600_even_when_overwriting() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_path("mode");
        let state = sample_state();
        fs::write(&path, b"old").expect("seed");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod");

        state.write_to_path(&path).expect("write");
        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "exported credentials must be owner-only");
        let content = fs::read_to_string(&path).expect("read");
        assert!(
            content.contains("TARGET_SECRET"),
            "content must stay usable"
        );
        let _ = fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_target_is_rejected() {
        use std::os::unix::fs::symlink;

        let path = temp_path("symlink");
        let link = temp_path("symlink-link");
        fs::write(&path, b"{}").expect("seed");
        symlink(&path, &link).expect("symlink");
        let error = sample_state().write_to_path(&link).expect_err("reject");
        assert!(matches!(error, StorageStateError::UnsafePath(_)), "{error}");
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&link);
    }

    #[cfg(not(unix))]
    #[test]
    fn writing_is_unsupported_without_unix_permissions() {
        let path = temp_path("platform");
        let error = sample_state()
            .write_to_path(&path)
            .expect_err("unsupported");
        assert!(matches!(error, StorageStateError::UnsupportedPlatform(_)));
        assert_eq!(error.code(), ErrorCode::Unsupported);
        assert!(!path.exists());
    }

    #[test]
    fn session_state_export_keeps_only_matching_origins() {
        let mut session = SessionState::new("default", std::time::Duration::from_secs(60));
        session
            .cookies
            .push(stored("sid", "SESSION_SECRET", "example.test"));
        session.note_origin("https://example.test");
        session.note_origin("https://other.example");

        let state = export_session_storage_state(&session, "example.test").expect("export");
        assert_eq!(state.cookies.len(), 1);
        assert_eq!(state.origins.len(), 1);
        assert_eq!(state.origins[0].origin, "https://example.test");
        assert!(state.origins[0].local_storage.is_empty());
    }
}
