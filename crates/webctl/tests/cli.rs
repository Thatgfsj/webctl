//! CLI-01…CLI-11 的端到端集成测试（离线：只连 127.0.0.1 的本地 fixture）。
//!
//! 用例编号对齐 docs/TESTING.md §2.12。所有测试通过运行已构建的 webctl 二进制，
//! 断言真实的输出、退出码与 fixture 请求计数。

mod support;

use std::net::TcpListener;
use std::process::Command;

use support::{Fixture, TempFile, missing_path, path_str};

/// 受控启动 webctl：移除宿主代理与 webctl 环境变量，并给一个不存在的默认配置路径。
fn webctl() -> Command {
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
    command.env("WEBCTL_CONFIG", missing_path("default"));
    command
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
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

fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind closed port");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    port
}

/// CLI-01：CLI 参数 > 环境变量 > 配置文件 > 默认（四层）。
#[test]
fn cli_01_config_precedence_four_layers() {
    let file = TempFile::new(
        "precedence",
        "timeout_ms = 111\nproxy = \"http://127.0.0.1:9001\"\n",
    );

    // 第 1 层：内置默认。
    let out = run(&mut webctl(), &["--json", "config"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let value = parse_json(&out);
    assert_eq!(value["data"]["timeout_ms"], 30_000);
    assert!(value["data"]["proxy"].is_null());

    // 第 2 层：配置文件覆盖默认。
    let mut command = webctl();
    command.env_remove("WEBCTL_CONFIG");
    let out = run(
        &mut command,
        &["--config", &path_str(file.path()), "--json", "config"],
    );
    let value = parse_json(&out);
    assert_eq!(value["data"]["timeout_ms"], 111);
    assert_eq!(value["data"]["proxy"], "http://127.0.0.1:9001");

    // 第 3 层：环境变量覆盖配置文件。
    let mut command = webctl();
    command
        .env("WEBCTL_TIMEOUT_MS", "222")
        .env("WEBCTL_OFFLINE", "true")
        .env("WEBCTL_PROXY", "http://127.0.0.1:9002");
    let out = run(
        &mut command,
        &["--config", &path_str(file.path()), "--json", "config"],
    );
    let value = parse_json(&out);
    assert_eq!(value["data"]["timeout_ms"], 222);
    assert_eq!(value["data"]["offline"], true);
    assert_eq!(value["data"]["proxy"], "http://127.0.0.1:9002");

    // 第 4 层：CLI 参数覆盖环境变量。
    let mut command = webctl();
    command
        .env("WEBCTL_TIMEOUT_MS", "222")
        .env("WEBCTL_PROXY", "http://127.0.0.1:9002");
    let out = run(
        &mut command,
        &[
            "--config",
            &path_str(file.path()),
            "--json",
            "--timeout-ms",
            "333",
            "--proxy",
            "http://127.0.0.1:9003",
            "config",
        ],
    );
    let value = parse_json(&out);
    assert_eq!(value["data"]["timeout_ms"], 333);
    assert_eq!(value["data"]["proxy"], "http://127.0.0.1:9003");
}

/// CLI-04 的配置面：输出格式同样遵循四层优先级。
#[test]
fn cli_01_output_format_precedence() {
    let file = TempFile::new("output", "output = \"text\"\n");

    // 配置文件 text → 人类可读。
    let mut command = webctl();
    command.env_remove("WEBCTL_CONFIG");
    let out = run(
        &mut command,
        &["--config", &path_str(file.path()), "config"],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout.starts_with("state_dir ="),
        "文件层 text 未生效: {}",
        out.stdout
    );

    // 环境变量 json → 覆盖文件 text。
    let mut command = webctl();
    command.env("WEBCTL_OUTPUT", "json");
    let out = run(
        &mut command,
        &["--config", &path_str(file.path()), "config"],
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(out.stdout.trim()).is_ok(),
        "环境变量 json 应覆盖文件 text: {}",
        out.stdout
    );

    // CLI --output text → 覆盖环境变量。
    let mut command = webctl();
    command.env("WEBCTL_OUTPUT", "json");
    let out = run(
        &mut command,
        &[
            "--config",
            &path_str(file.path()),
            "--output",
            "text",
            "config",
        ],
    );
    assert!(
        out.stdout.starts_with("state_dir ="),
        "--output text 应覆盖环境变量: {}",
        out.stdout
    );
}

/// CLI-02：配置文件语法/字段错误与非法环境变量都要明确报错并非零退出。
#[test]
fn cli_02_config_errors_exit_nonzero() {
    let syntax = TempFile::new("syntax", "output = [\n");
    let mut command = webctl();
    command.env_remove("WEBCTL_CONFIG");
    let out = run(
        &mut command,
        &["--config", &path_str(syntax.path()), "--json", "config"],
    );
    assert_eq!(out.code, 6, "stderr={}", out.stderr);
    let value = parse_json(&out);
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "PARSE");
    assert!(
        value["error"].as_str().unwrap().contains("解析失败"),
        "{}",
        value["error"]
    );

    let typo = TempFile::new("typo", "outpt = \"json\"\n");
    let out = run(
        &mut webctl(),
        &["--config", &path_str(typo.path()), "--json", "config"],
    );
    assert_eq!(out.code, 6);
    assert_eq!(parse_json(&out)["code"], "PARSE");

    // 显式指定的文件不存在：不得静默回落。
    let missing = missing_path("explicit");
    let out = run(
        &mut webctl(),
        &["--config", &path_str(&missing), "--json", "config"],
    );
    assert_eq!(out.code, 6);
    assert!(
        parse_json(&out)["error"]
            .as_str()
            .unwrap()
            .contains("文件不存在")
    );

    // 非法环境变量值（CLI-11）。
    let mut command = webctl();
    command.env("WEBCTL_TIMEOUT_MS", "soon");
    let out = run(&mut command, &["--json", "config"]);
    assert_eq!(out.code, 6);
    assert!(
        parse_json(&out)["error"]
            .as_str()
            .unwrap()
            .contains("WEBCTL_TIMEOUT_MS")
    );
}

/// CLI-03：配置文件代理与环境变量代理都生效（fixture 代理计数）。
#[test]
fn cli_03_proxy_from_config_and_env_is_used() {
    let proxy = Fixture::start("proxied");
    let file = TempFile::new("proxy", &format!("proxy = \"{}\"\n", proxy.base()));

    let mut command = webctl();
    command.env_remove("WEBCTL_CONFIG");
    let out = run(
        &mut command,
        &[
            "--config",
            &path_str(file.path()),
            "--json",
            "fetch",
            "http://example.invalid/x",
        ],
    );
    assert_eq!(out.code, 0, "stderr={}", out.stderr);
    assert_eq!(parse_json(&out)["ok"], true);
    assert_eq!(proxy.hits(), 1, "配置文件代理应被使用");

    let mut command = webctl();
    command.env("WEBCTL_PROXY", proxy.base());
    let out = run(
        &mut command,
        &["--json", "fetch", "http://example.invalid/y"],
    );
    assert_eq!(out.code, 0, "stderr={}", out.stderr);
    assert_eq!(parse_json(&out)["ok"], true);
    assert_eq!(proxy.hits(), 2, "环境变量代理应被使用");
}

/// CLI-03：未配置代理时必须直连，且不受宿主 HTTP_PROXY/HTTPS_PROXY 影响。
#[test]
fn cli_03_host_proxy_env_is_ignored() {
    let origin = Fixture::start("direct");
    let mut command = webctl();
    command
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .env("ALL_PROXY", "http://127.0.0.1:1");
    let out = run(&mut command, &["--json", "fetch", &origin.url("/ok")]);
    assert_eq!(out.code, 0, "未配置代理时必须直连: {}", out.stderr);
    assert_eq!(parse_json(&out)["ok"], true);
    assert!(origin.hits() >= 1, "直连请求必须到达源站");

    let mut command = webctl();
    command.env("HTTP_PROXY", "http://127.0.0.1:1");
    let out = run(&mut command, &["--json", "config"]);
    assert!(
        parse_json(&out)["data"]["proxy"].is_null(),
        "宿主 HTTP_PROXY 不得进入配置"
    );
}

/// CLI-04：--output json|text 与 --json 可切换，JSON 可被解析器消费。
#[test]
fn cli_04_output_format_switches() {
    let out = run(&mut webctl(), &["caps", "--json"]);
    assert_eq!(out.code, 0);
    let value = parse_json(&out);
    assert_eq!(value["data"]["capabilities"].as_array().unwrap().len(), 5);
    assert_eq!(value["meta"]["trust"], "trusted_local");

    let out = run(&mut webctl(), &["caps", "--output", "text"]);
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("webctl "), "text 输出: {}", out.stdout);
    assert!(out.stdout.contains("fetch"));
    assert!(serde_json::from_str::<serde_json::Value>(out.stdout.trim()).is_err());

    let out = run(&mut webctl(), &["caps", "--output", "text", "--json"]);
    assert!(
        serde_json::from_str::<serde_json::Value>(out.stdout.trim()).is_ok(),
        "--json 必须覆盖 --output",
    );
}

/// CLI-05：错误码 → 退出码在真实进程上稳定。
#[test]
fn cli_05_exit_codes_are_stable() {
    assert_eq!(run(&mut webctl(), &["caps", "--json"]).code, 0);
    assert_eq!(
        run(&mut webctl(), &["fetch", "ftp://example.invalid/"]).code,
        2
    );
    assert_eq!(
        run(
            &mut webctl(),
            &["--offline", "fetch", "http://example.invalid/"]
        )
        .code,
        5
    );
    let missing = missing_path("exit");
    assert_eq!(
        run(&mut webctl(), &["--config", &path_str(&missing), "config"]).code,
        6
    );

    let port = closed_port();
    let out = run(
        &mut webctl(),
        &["--json", "fetch", &format!("http://127.0.0.1:{port}/")],
    );
    assert_eq!(out.code, 4, "连接失败应为 NETWORK: {}", out.stdout);
    assert_eq!(parse_json(&out)["code"], "NETWORK");
}

/// CLI-06：--offline 下零网络访问（fixture 计数为 0）。
#[test]
fn cli_06_offline_makes_zero_requests() {
    let origin = Fixture::start("offline");
    let out = run(
        &mut webctl(),
        &["--offline", "--json", "fetch", &origin.url("/ok")],
    );
    assert_eq!(out.code, 5);
    assert_eq!(parse_json(&out)["code"], "BLOCKED");
    assert_eq!(origin.hits(), 0, "offline 必须零网络访问");

    let mut command = webctl();
    command.env("WEBCTL_OFFLINE", "1");
    let out = run(&mut command, &["--json", "fetch", &origin.url("/again")]);
    assert_eq!(out.code, 5);
    assert_eq!(origin.hits(), 0, "环境变量 offline 也必须零网络访问");
}

/// CLI-07 / CLI-11：--help 列出全部子命令与环境变量。
#[test]
fn cli_07_help_lists_subcommands_and_env_vars() {
    let out = run(&mut webctl(), &["--help"]);
    assert_eq!(out.code, 0);
    for name in ["caps", "fetch", "config"] {
        assert!(out.stdout.contains(name), "帮助缺少子命令 {name}");
    }
    for key in webctl_core::ENV_VARS {
        assert!(out.stdout.contains(key), "帮助缺少环境变量 {key}");
    }
}

/// CLI-08：未知参数非零退出 + 可读提示。
#[test]
fn cli_08_unknown_args_fail_with_message() {
    let out = run(&mut webctl(), &["--definitely-not-a-flag"]);
    assert_ne!(out.code, 0);
    assert!(
        out.stderr.contains("--definitely-not-a-flag"),
        "stderr={}",
        out.stderr
    );
    assert!(
        out.stderr.to_lowercase().contains("error"),
        "stderr={}",
        out.stderr
    );

    let out = run(&mut webctl(), &["fetch"]);
    assert_ne!(out.code, 0);
    assert!(
        out.stderr.contains("URL") || out.stderr.contains("required"),
        "stderr={}",
        out.stderr
    );
}

/// CLI-09：单行 JSON 可直接管道给下游工具。
#[test]
fn cli_09_json_is_single_line() {
    let out = run(&mut webctl(), &["caps", "--json"]);
    assert_eq!(out.code, 0);
    assert_eq!(
        out.stdout.trim().lines().count(),
        1,
        "JSON 必须是单行: {:?}",
        out.stdout
    );
    assert!(serde_json::from_str::<serde_json::Value>(out.stdout.trim()).is_ok());
}

/// CLI-10：任何输出都不含凭据（cookie 明文、代理口令）。
#[test]
fn cli_10_no_credentials_in_output() {
    let origin = Fixture::start("secret");
    let out = run(&mut webctl(), &["--json", "fetch", &origin.url("/secret")]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let combined = format!("{}{}", out.stdout, out.stderr);
    assert!(
        !combined.contains(support::SECRET_COOKIE_VALUE),
        "响应 cookie 不得出现在输出: {combined}"
    );
    let value = parse_json(&out);
    let headers = value["data"]["headers"].as_array().unwrap();
    assert!(
        headers.iter().any(|header| {
            header[0]
                .as_str()
                .unwrap()
                .eq_ignore_ascii_case("set-cookie")
                && header[1] == "<redacted>"
        }),
        "set-cookie 必须被脱敏: {headers:?}"
    );

    // 代理凭据在 config 输出里被掩码。
    let out = run(
        &mut webctl(),
        &[
            "--json",
            "--proxy",
            "http://user:hunter2@127.0.0.1:1",
            "config",
        ],
    );
    let value = parse_json(&out);
    assert_eq!(value["data"]["proxy"], "http://***@127.0.0.1:1");
    assert!(!format!("{}{}", out.stdout, out.stderr).contains("hunter2"));

    // 代理失败时错误信息也不得泄漏凭据。
    let port = closed_port();
    let out = run(
        &mut webctl(),
        &[
            "--json",
            "--proxy",
            "http://user:hunter2@127.0.0.1:1",
            "fetch",
            &format!("http://127.0.0.1:{port}/"),
        ],
    );
    assert!(
        !format!("{}{}", out.stdout, out.stderr).contains("hunter2"),
        "代理凭据不得出现在错误输出"
    );
}

/// CLI-11：全部环境变量在 docs/CONFIGURATION.md 中有记录。
#[test]
fn cli_11_env_vars_are_documented() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/CONFIGURATION.md");
    let doc = std::fs::read_to_string(path).expect("read docs/CONFIGURATION.md");
    for key in webctl_core::ENV_VARS {
        assert!(doc.contains(key), "文档缺少环境变量 {key}");
    }
    let readme_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md");
    let readme = std::fs::read_to_string(readme_path).expect("read README.md");
    assert!(
        readme.contains("CONFIGURATION.md"),
        "README 必须链接配置文档"
    );
}

// ---------------------------------------------------------------------------
// #183：浏览器上下文的 CLI 开关（--browser-context / --no-browser-context）
// ---------------------------------------------------------------------------

const CONTEXT_JSON: &str = r#"{"user_agent":"ctx-agent/1.0","accept_language":"zh-CN","cookies":[{"domain":"127.0.0.1","name":"sid","value":"CTX"}]}"#;

fn context_file(tag: &str) -> TempFile {
    TempFile::new(tag, CONTEXT_JSON)
}

/// #183：--browser-context 把 UA / accept-language / cookie 注入到 CLI fetch。
#[test]
fn browser_context_flag_injects_headers_on_fetch() {
    let fixture = Fixture::start("ok");
    let context = context_file("ctx-flag");
    let context_path = path_str(context.path());
    let url = fixture.url("/page");
    let result = run(
        &mut webctl(),
        &["--browser-context", &context_path, "fetch", &url],
    );
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    let head = fixture.last_head().to_ascii_lowercase();
    assert!(
        head.contains("user-agent: ctx-agent/1.0"),
        "缺少 UA：{head}"
    );
    assert!(
        head.contains("accept-language: zh-cn"),
        "缺少 accept-language：{head}"
    );
    assert!(head.contains("sid=ctx"), "缺少上下文 cookie：{head}");
}

/// #183：--no-browser-context 压过 WEBCTL_BROWSER_CONTEXT（显式优于环境）。
#[test]
fn no_browser_context_overrides_env() {
    let fixture = Fixture::start("ok");
    let context = context_file("ctx-off");
    let mut command = webctl();
    command.env("WEBCTL_BROWSER_CONTEXT", path_str(context.path()));
    let result = run(
        &mut command,
        &["--no-browser-context", "fetch", &fixture.url("/page")],
    );
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    let head = fixture.last_head().to_ascii_lowercase();
    assert!(!head.contains("ctx-agent"), "不应注入 UA：{head}");
    assert!(!head.contains("sid=ctx"), "不应注入 cookie：{head}");
}

/// #183：未给开关时仍认环境变量（既有行为不回退）。
#[test]
fn env_browser_context_is_still_honored() {
    let fixture = Fixture::start("ok");
    let context = context_file("ctx-env");
    let mut command = webctl();
    command.env("WEBCTL_BROWSER_CONTEXT", path_str(context.path()));
    let result = run(&mut command, &["fetch", &fixture.url("/page")]);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    let head = fixture.last_head().to_ascii_lowercase();
    assert!(
        head.contains("user-agent: ctx-agent/1.0"),
        "缺少 UA：{head}"
    );
}

/// #183：两个开关互斥（clap 参数错误 → 退出码 2）。
#[test]
fn browser_context_flags_are_mutually_exclusive() {
    let result = run(
        &mut webctl(),
        &[
            "--browser-context",
            "ctx.json",
            "--no-browser-context",
            "caps",
        ],
    );
    assert_eq!(
        result.code, 2,
        "stdout={} stderr={}",
        result.stdout, result.stderr
    );
    assert!(
        result.stderr.contains("cannot be used with"),
        "clap 应报互斥：{}",
        result.stderr
    );
}

/// #183：显式给的文件坏了必须**失败**，不静默忽略（与 MCP 环境变量的宽容口径不同）。
#[test]
fn broken_browser_context_file_fails_loudly() {
    let broken = TempFile::new("ctx-bad", "{ this is not json");
    let broken_path = path_str(broken.path());
    let result = run(&mut webctl(), &["--browser-context", &broken_path, "caps"]);
    assert_ne!(result.code, 0, "坏文件不得静默成功");
    assert!(
        result.stderr.contains("浏览器上下文") || result.stdout.contains("BAD_URL"),
        "必须给出可读原因：stdout={} stderr={}",
        result.stdout,
        result.stderr
    );
}
