//! #240：`webctl cookies storage-state` 的离线 CLI 契约测试。
//!
//! 所有用例只操作临时 state dir 与本地文件；没有任何网络访问。

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use support::{missing_path, path_str};

/// 只应出现在导出文件里的 cookie 值。
const SECRET: &str = "STORAGE_STATE_SECRET";

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "webctl-ss-{tag}-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn webctl(state: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_webctl"));
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "WEBCTL_STATE_DIR",
        "WEBCTL_OUTPUT",
        "WEBCTL_TIMEOUT_MS",
        "WEBCTL_OFFLINE",
        "WEBCTL_PROXY",
        "WEBCTL_BROWSER_CONTEXT",
        "XDG_CONFIG_HOME",
    ] {
        command.env_remove(key);
    }
    command.env("WEBCTL_CONFIG", missing_path("ss-default"));
    command.env("WEBCTL_STATE_DIR", state);
    command
}

fn run(command: &mut Command, args: &[&str]) -> Run {
    let output = command.args(args).output().expect("spawn webctl");
    Run {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn parse_json(run: &Run) -> serde_json::Value {
    serde_json::from_str(run.stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "stdout 不是 JSON: {error}\nstdout={}\nstderr={}",
            run.stdout, run.stderr
        )
    })
}

fn cookie(name: &str, value: &str, domain: &str) -> String {
    format!(
        r#"{{"name":"{name}","value":"{value}","domain":"{domain}","host_only":true,"path":"/","secure":true,"http_only":true,"expires_at":null}}"#
    )
}

fn seed_session(state: &Path, cookies: &[String], origins: &[&str]) {
    let sessions = state.join("sessions");
    std::fs::create_dir_all(&sessions).expect("create sessions dir");
    let origins = origins
        .iter()
        .map(|origin| format!("\"{origin}\""))
        .collect::<Vec<_>>()
        .join(",");
    let json = format!(
        "{{\"version\":1,\"name\":\"default\",\"created_at\":1,\"updated_at\":1,\"expires_at\":null,\"cookies\":[{}],\"origins\":[{}]}}",
        cookies.join(","),
        origins
    );
    std::fs::write(sessions.join("default.json"), json).expect("seed session");
}

fn export_args<'a>(file: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "--json",
        "cookies",
        "storage-state",
        "--session",
        "default",
        "--file",
        file,
        "--yes",
    ];
    args.extend_from_slice(extra);
    args
}

/// 成功导出：Playwright 结构、字段大小写、文件保留原值、输出不含明文。
#[cfg(unix)]
#[test]
fn storage_state_json_matches_playwright_shape() {
    let state = TempDir::new("shape");
    seed_session(
        state.path(),
        &[cookie("sid", SECRET, "example.test")],
        &["https://example.test"],
    );
    let args = export_args("state.json", &["--domain", "example.test"]);
    let out = run(&mut webctl(state.path()), &args);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let value = parse_json(&out);
    assert_eq!(value["code"], "OK", "{value}");
    assert_eq!(value["data"]["target_domain"], "example.test");
    assert_eq!(value["data"]["cookies"], 1);
    assert_eq!(value["data"]["origins"], 1);
    assert_eq!(value["data"]["mode"], serde_json::json!(0o600));
    let combined = format!("{}{}", out.stdout, out.stderr);
    assert!(
        !combined.contains(SECRET),
        "信封/日志不得含 cookie 值：{combined}"
    );

    let raw = std::fs::read_to_string(state.path().join("state.json")).expect("exported file");
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
    let cookie = &parsed["cookies"][0];
    for field in [
        "name", "value", "domain", "path", "expires", "httpOnly", "secure", "sameSite",
    ] {
        assert!(
            cookie.get(field).is_some(),
            "缺少 Playwright 字段 {field}: {cookie}"
        );
    }
    assert!(
        cookie.get("http_only").is_none(),
        "字段名必须逐字对齐 Playwright"
    );
    assert_eq!(
        cookie["value"], SECRET,
        "落盘必须保留原值，否则无法复用登录态"
    );
    assert_eq!(cookie["sameSite"], "Lax");
    assert_eq!(cookie["expires"], -1);
    assert_eq!(parsed["origins"][0]["origin"], "https://example.test");
    assert!(
        parsed["origins"][0]["localStorage"]
            .as_array()
            .expect("localStorage array")
            .is_empty()
    );
}

/// 未给 --domain 时，默认目标域覆盖会话观察到的同站域，且只导出这个域。
#[cfg(unix)]
#[test]
fn default_target_domain_is_inferred_and_limited() {
    let state = TempDir::new("infer");
    seed_session(
        state.path(),
        &[
            cookie("root", SECRET, "example.test"),
            cookie("sub", SECRET, "app.example.test"),
        ],
        &["https://app.example.test/login"],
    );
    let args = export_args("state.json", &[]);
    let out = run(&mut webctl(state.path()), &args);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let value = parse_json(&out);
    assert_eq!(value["data"]["target_domain"], "example.test", "{value}");
    assert_eq!(value["data"]["cookies"], 2, "{value}");
}

/// 显式 --domain 时，非目标域 cookie 不得进入文件。
#[cfg(unix)]
#[test]
fn non_target_cookie_is_not_exported() {
    let state = TempDir::new("filter");
    seed_session(
        state.path(),
        &[
            cookie("sid", SECRET, "example.test"),
            cookie("other", "OTHER_SECRET", "other.test"),
        ],
        &["https://example.test"],
    );
    let args = export_args("state.json", &["--domain", "example.test"]);
    let out = run(&mut webctl(state.path()), &args);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let raw = std::fs::read_to_string(state.path().join("state.json")).expect("exported file");
    assert!(raw.contains(SECRET));
    assert!(
        !raw.contains("OTHER_SECRET"),
        "非目标域 cookie 不得落盘：{raw}"
    );
    assert!(!raw.contains("other.test"));
}

/// 文件权限必须是 0600（不是 0644，也不能只在信封里声称）。
#[cfg(unix)]
#[test]
fn storage_state_file_is_mode_0600() {
    use std::os::unix::fs::PermissionsExt;

    let state = TempDir::new("mode");
    seed_session(
        state.path(),
        &[cookie("sid", SECRET, "example.test")],
        &["https://example.test"],
    );
    let args = export_args("state.json", &["--domain", "example.test"]);
    let out = run(&mut webctl(state.path()), &args);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let mode = std::fs::metadata(state.path().join("state.json"))
        .expect("exported file")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

/// 非 Unix 明确 Unsupported，绝不降级成 0644 或静默成功。
#[cfg(not(unix))]
#[test]
fn storage_state_is_unsupported_without_unix_permissions() {
    let state = TempDir::new("platform");
    seed_session(
        state.path(),
        &[cookie("sid", SECRET, "example.test")],
        &["https://example.test"],
    );
    let args = export_args("state.json", &["--domain", "example.test"]);
    let out = run(&mut webctl(state.path()), &args);
    assert_eq!(out.code, 8, "{}{}", out.stdout, out.stderr);
    assert_eq!(parse_json(&out)["code"], "UNSUPPORTED");
    assert!(!state.path().join("state.json").exists());
}

/// 路径穿越、绝对路径、目录、symlink 一律拒绝且不产生副作用。
#[test]
fn unsafe_paths_are_rejected() {
    let state = TempDir::new("paths");
    seed_session(
        state.path(),
        &[cookie("sid", SECRET, "example.test")],
        &["https://example.test"],
    );

    let args = export_args("../escape.json", &["--domain", "example.test"]);
    let out = run(&mut webctl(state.path()), &args);
    assert_ne!(out.code, 0);
    assert_eq!(parse_json(&out)["code"], "BLOCKED");
    assert!(
        !state
            .path()
            .parent()
            .expect("parent")
            .join("escape.json")
            .exists()
    );

    let absolute = state.path().join("absolute.json");
    let absolute_arg = path_str(&absolute);
    let args = export_args(&absolute_arg, &["--domain", "example.test"]);
    let out = run(&mut webctl(state.path()), &args);
    assert_eq!(parse_json(&out)["code"], "BLOCKED");
    assert!(!absolute.exists());

    std::fs::create_dir(state.path().join("export-dir")).expect("dir");
    let args = export_args("export-dir", &["--domain", "example.test"]);
    let out = run(&mut webctl(state.path()), &args);
    assert_eq!(parse_json(&out)["code"], "BLOCKED");

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = TempDir::new("paths-outside");
        let victim = outside.path().join("victim.json");
        std::fs::write(&victim, b"keep").expect("victim");
        symlink(&victim, state.path().join("link.json")).expect("symlink");
        let args = export_args("link.json", &["--domain", "example.test"]);
        let out = run(&mut webctl(state.path()), &args);
        assert_eq!(parse_json(&out)["code"], "BLOCKED");
        assert_eq!(std::fs::read(&victim).expect("victim"), b"keep");
    }
}

/// 确认、会话、域与参数错误各自给出稳定错误码/退出码。
#[test]
fn confirmation_and_parameter_errors() {
    let state = TempDir::new("errors");
    seed_session(
        state.path(),
        &[cookie("sid", SECRET, "example.test")],
        &["https://example.test"],
    );

    // 未确认：BLOCKED（5），且不创建文件。
    let out = run(
        &mut webctl(state.path()),
        &[
            "--json",
            "cookies",
            "storage-state",
            "--session",
            "default",
            "--file",
            "state.json",
            "--domain",
            "example.test",
        ],
    );
    assert_eq!(out.code, 5);
    assert_eq!(parse_json(&out)["code"], "BLOCKED");
    assert!(!state.path().join("state.json").exists());

    // 缺少 --file：clap 参数错误退出码 2。
    let out = run(
        &mut webctl(state.path()),
        &["cookies", "storage-state", "--session", "default", "--yes"],
    );
    assert_eq!(out.code, 2);

    // 非法目标域：BLOCKED，不落盘。
    let args = export_args("bad-domain.json", &["--domain", "https://example.test"]);
    let out = run(&mut webctl(state.path()), &args);
    assert_eq!(out.code, 5);
    assert_eq!(parse_json(&out)["code"], "BLOCKED");
    assert!(!state.path().join("bad-domain.json").exists());

    // 会话不存在：SESSION_LOST（9）。
    let out = run(
        &mut webctl(state.path()),
        &[
            "--json",
            "cookies",
            "storage-state",
            "--session",
            "missing",
            "--file",
            "state.json",
            "--domain",
            "example.test",
            "--yes",
        ],
    );
    assert_eq!(out.code, 9);
    assert_eq!(parse_json(&out)["code"], "SESSION_LOST");
}

/// `--offline` 对纯本地导出不应产生副作用（不联网也不阻止导出）。
#[cfg(unix)]
#[test]
fn offline_flag_does_not_block_local_export() {
    let state = TempDir::new("offline");
    seed_session(
        state.path(),
        &[cookie("sid", SECRET, "example.test")],
        &["https://example.test"],
    );
    let args = export_args("state.json", &["--domain", "example.test"]);
    let mut command = webctl(state.path());
    let mut offline_args = vec!["--offline"];
    offline_args.extend_from_slice(&args);
    let out = run(&mut command, &offline_args);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(state.path().join("state.json").exists());
}

/// 兼容性：既有 `cookies list/export` 仍在命令树上。
#[test]
fn existing_cookies_subcommands_remain() {
    let out = run(&mut webctl(Path::new(".")), &["cookies", "--help"]);
    let text = format!("{}{}", out.stdout, out.stderr);
    for expected in ["list", "export", "storage-state", "export-storage-state"] {
        assert!(
            text.contains(expected),
            "cookies --help 缺少 {expected}: {text}"
        );
    }
}
