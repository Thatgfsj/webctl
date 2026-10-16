//! PR 12 安全负面用例（SEC-01 / 02 / 03 / 05 / 11 / 13 / 15）。
//!
//! 全部以**默认配置**运行真实二进制（CARGO_BIN_EXE_webctl），不加任何放宽开关；
//! 只连 127.0.0.1 本地 fixture。断言用正则/结构化比较，且都能失败（见植入实验）。
//!
//! SEC-09（MCP trust）归 PR 11；SEC-10（无 shell）/SEC-12（状态权限）由 PR 13/PR 05 覆盖。

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;

/// 注入的机密值：任何输出出现它即判失败。
const SECRET: &str = "SUPER_SECRET_TOKEN";

/// 临时 state dir，drop 时递归删除。
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "webctl-cli-sec-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create temp state dir");
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

/// 受控启动 webctl：清掉宿主代理与 webctl 环境变量，指向临时 state dir。
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
        "XDG_CONFIG_HOME",
    ] {
        command.env_remove(key);
    }
    command.env("WEBCTL_CONFIG", support::missing_path("sec-default"));
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

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("测试正则必须可编译")
}

/// 在 state dir 里写一个含机密 cookie 的会话文件。
fn seed_session(state: &Path) {
    let sessions = state.join("sessions");
    std::fs::create_dir_all(&sessions).expect("create sessions dir");
    let json = format!(
        r#"{{"version":1,"name":"default","created_at":1,"updated_at":1,"expires_at":null,"cookies":[{{"name":"sid","value":"{SECRET}","domain":"example.test","host_only":true,"path":"/","secure":true,"http_only":true,"expires_at":null}}],"origins":[]}}"#
    );
    std::fs::write(sessions.join("default.json"), json).expect("seed session");
}

/// SEC-01 + SEC-02：默认严格；导出无 `--yes` 被拒且不落盘。
#[test]
fn sec_01_sec_02_default_strict_and_export_confirmation() {
    let state = TempDir::new("sec01");
    let help = run(&mut webctl(state.path()), &["--help"]);
    let help_text = format!("{}{}", help.stdout, help.stderr);
    assert!(
        !regex(r"--(allow|insecure|no-verify|no-robots|dangerous|unsafe)").is_match(&help_text),
        "帮助里不得出现放宽开关：{help_text}"
    );

    seed_session(state.path());
    let out = run(
        &mut webctl(state.path()),
        &[
            "--json",
            "cookies",
            "export",
            "--session",
            "default",
            "--file",
            "cookies.json",
        ],
    );
    assert_ne!(out.code, 0, "无 --yes 必须非零退出");
    let value = parse_json(&out);
    assert_eq!(value["code"], "BLOCKED", "{value}");
    assert!(
        value["error"].as_str().unwrap_or("").contains("--yes"),
        "{value}"
    );
    assert!(
        !state.path().join("cookies.json").exists(),
        "未确认时不得创建导出文件"
    );
}

/// SEC-03：导出文件权限 600（envelope 与文件系统双向验证）。
#[test]
fn sec_03_export_file_is_mode_600() {
    let state = TempDir::new("sec03");
    seed_session(state.path());
    let out = run(
        &mut webctl(state.path()),
        &[
            "--json",
            "cookies",
            "export",
            "--session",
            "default",
            "--file",
            "cookies.json",
            "--yes",
        ],
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    let value = parse_json(&out);
    assert_eq!(value["data"]["mode"], serde_json::json!(0o600), "{value}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(state.path().join("cookies.json"))
            .expect("exported file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

/// SEC-05：cookie 明文不落输出/日志（fetch 信封 + cookies 清单）。
#[test]
fn sec_05_cookie_values_never_reach_output_or_logs() {
    let state = TempDir::new("sec05");
    let origin = support::Fixture::start("secret");
    let out = run(
        &mut webctl(state.path()),
        &["--json", "fetch", &origin.url("/secret")],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let combined = format!("{}{}", out.stdout, out.stderr);
    assert!(
        !combined.contains(support::SECRET_COOKIE_VALUE),
        "cookie 明文不得出现在输出/日志：{combined}"
    );
    assert!(regex("<redacted>").is_match(&combined), "{combined}");

    seed_session(state.path());
    let listed = run(
        &mut webctl(state.path()),
        &["--json", "cookies", "list", "--session", "default"],
    );
    let listed_text = format!("{}{}", listed.stdout, listed.stderr);
    assert!(!listed_text.contains(SECRET), "{listed_text}");
    assert!(regex("<redacted>").is_match(&listed_text), "{listed_text}");
}

/// SEC-07：URL query token 在日志/错误信息里脱敏。
#[test]
fn sec_07_url_query_token_is_redacted_in_logs() {
    let state = TempDir::new("sec07");
    let url = format!("http://127.0.0.1:1/x?token={SECRET}&user=bob");
    let out = run(&mut webctl(state.path()), &["--json", "fetch", &url]);
    assert_ne!(out.code, 0);
    let combined = format!("{}{}", out.stdout, out.stderr);
    assert!(!combined.contains(SECRET), "{combined}");
    assert!(
        regex(r"token=<redacted>").is_match(&out.stderr),
        "stderr 应含脱敏后的 token：{}",
        out.stderr
    );
    assert!(
        !combined.contains("<redacted><redacted>"),
        "不得出现重复占位：{combined}"
    );
}

/// SEC-11：CLI 所有文件参数经 containment；`../`、绝对路径、越界 symlink、坏会话名全部拒绝。
#[test]
fn sec_11_cli_file_arguments_are_contained() {
    let state = TempDir::new("sec11");
    seed_session(state.path());

    // ../ 穿越
    let out = run(
        &mut webctl(state.path()),
        &[
            "--json",
            "cookies",
            "export",
            "--session",
            "default",
            "--file",
            "../escape.json",
            "--yes",
        ],
    );
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

    // 绝对路径
    let absolute = state.path().join("absolute.json");
    let absolute_arg = absolute.to_string_lossy().into_owned();
    let out = run(
        &mut webctl(state.path()),
        &[
            "--json",
            "cookies",
            "export",
            "--session",
            "default",
            "--file",
            &absolute_arg,
            "--yes",
        ],
    );
    assert_ne!(out.code, 0);
    assert_eq!(parse_json(&out)["code"], "BLOCKED");
    assert!(!absolute.exists());

    // 坏会话名（路径穿越）
    let out = run(
        &mut webctl(state.path()),
        &["--json", "cookies", "list", "--session", "../evil"],
    );
    assert_ne!(out.code, 0);
    assert_eq!(parse_json(&out)["code"], "BLOCKED");

    // 越界 symlink
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = TempDir::new("sec11-outside");
        let victim = outside.path().join("victim.json");
        std::fs::write(&victim, b"keep").expect("victim");
        symlink(&victim, state.path().join("link.json")).expect("symlink");
        let out = run(
            &mut webctl(state.path()),
            &[
                "--json",
                "cookies",
                "export",
                "--session",
                "default",
                "--file",
                "link.json",
                "--yes",
            ],
        );
        assert_ne!(out.code, 0);
        assert_eq!(parse_json(&out)["code"], "BLOCKED");
        assert_eq!(std::fs::read(&victim).expect("victim"), b"keep");
    }
}

/// SEC-13：错误输出不含堆栈、源文件路径栈帧或本地文件路径。
#[test]
fn sec_13_errors_carry_no_stack_or_paths() {
    let state = TempDir::new("sec13");
    let stack = regex(r"panicked|\.rs:[0-9]+|backtrace|RUST_BACKTRACE|/home/");

    let out = run(&mut webctl(state.path()), &["--json", "fetch", "not-a-url"]);
    assert_ne!(out.code, 0);
    let combined = format!("{}{}", out.stdout, out.stderr);
    assert!(!stack.is_match(&combined), "{combined}");

    let out = run(
        &mut webctl(state.path()),
        &["--json", "fetch", "file:///etc/passwd"],
    );
    assert_eq!(parse_json(&out)["code"], "BAD_URL");
    let combined = format!("{}{}", out.stdout, out.stderr);
    assert!(!combined.contains("/etc/passwd"), "{combined}");
    assert!(!stack.is_match(&combined), "{combined}");
}

/// SEC-15：`file://` 等非 http(s) 协议经 CLI 被拒（退出码 2 = BAD_URL）。
#[test]
fn sec_15_dangerous_scheme_via_cli() {
    let state = TempDir::new("sec15");
    for bad in ["file:///etc/passwd", "ftp://127.0.0.1/x"] {
        let out = run(&mut webctl(state.path()), &["--json", "fetch", bad]);
        assert_eq!(out.code, 2, "{bad}: {}{}", out.stdout, out.stderr);
        let value = parse_json(&out);
        assert_eq!(value["code"], "BAD_URL", "{value}");
    }
}
