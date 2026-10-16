//! webctl CLI 入口（PLAN v0.4 §3.7；TESTING CLI-01…CLI-12）。
//!
//! 职责边界：参数解析、四层配置装载、结果输出与退出码映射。
//! 能力实现全在 core / web crate 内；本进程不启动任何子进程（PLAN v0.4 §3.1）。
//!
//! 输出约定：JSON 恒为单行（CLI-09 管道友好）；text 为人类可读。
//! 失败也走结果信封，退出码由 exit_code 稳定映射（CLI-05）。
use std::future::Future;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use webctl_core::security::paths::{SafeRoot, write_secret_file};
use webctl_core::{
    CapabilityState, CapsReport, Config, ConfigError, ConfigLayer, ENV_VARS, Envelope, ErrorCode,
    OutputFormat, Trust, default_config_path, load_config_file, redact_url_userinfo, resolve,
};
use webctl_web::SessionStore;
use webctl_web::transport::{FetchOptions, Fetched, fetch};

/// 错误码 → 进程退出码的稳定映射（CLI-05）。
///
/// 取值一经发布不得更改；完整表格见 docs/CONFIGURATION.md 与 --help。
pub fn exit_code(code: ErrorCode) -> u8 {
    match code {
        ErrorCode::Ok => 0,
        ErrorCode::Internal => 1,
        ErrorCode::BadUrl => 2,
        ErrorCode::Timeout => 3,
        ErrorCode::Network => 4,
        ErrorCode::Blocked => 5,
        ErrorCode::Parse => 6,
        ErrorCode::Degraded => 7,
        ErrorCode::Unsupported => 8,
        ErrorCode::SessionLost => 9,
        ErrorCode::Limit => 10,
    }
}

/// CLI 输出格式参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputArg {
    /// 机器可读 JSON（默认）。
    Json,
    /// 人类可读文本。
    Text,
}

impl From<OutputArg> for OutputFormat {
    fn from(value: OutputArg) -> Self {
        match value {
            OutputArg::Json => Self::Json,
            OutputArg::Text => Self::Text,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "webctl",
    version,
    about = "给 AI agent 用的网页控制层：全部内置，零外部进程、零浏览器",
    long_about = None
)]
struct Cli {
    /// 配置文件路径（默认：WEBCTL_CONFIG > XDG_CONFIG_HOME/webctl/config.toml > ~/.config/webctl/config.toml > ./webctl.toml）。
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    /// 输出格式（默认 json）。
    #[arg(long, global = true, value_enum, value_name = "FORMAT")]
    output: Option<OutputArg>,

    /// 等价于 --output json；显式给出时覆盖 --output。
    #[arg(long, global = true)]
    json: bool,

    /// 离线模式：拒绝任何网络访问。
    #[arg(long, global = true)]
    offline: bool,

    /// 显式 HTTP(S) 代理 URL（也可用 WEBCTL_PROXY 或配置文件 proxy）。
    #[arg(long, global = true, value_name = "URL")]
    proxy: Option<String>,

    /// 单次请求超时（毫秒）。
    #[arg(long, global = true, value_name = "MS")]
    timeout_ms: Option<u64>,

    #[command(subcommand)]
    command: Command,
}

impl Cli {
    /// 只把显式给出的参数放进 CLI 层（未给出的字段继续沿用低优先级的值）。
    fn to_layer(&self) -> ConfigLayer {
        ConfigLayer {
            output: if self.json {
                Some(OutputFormat::Json)
            } else {
                self.output.map(OutputFormat::from)
            },
            timeout_ms: self.timeout_ms,
            offline: self.offline.then_some(true),
            proxy: self.proxy.clone(),
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 内置能力自检（不联网、不探测外部程序）。
    Caps,
    /// 取回一个 http(s) URL，输出结果信封。
    Fetch {
        /// 目标 URL。
        url: String,
    },
    /// 打印生效配置（代理凭据已脱敏）。
    Config,
    /// 查看/导出 cookie（默认严格：查看值脱敏；导出需 --yes、文件 0600）。
    Cookies {
        /// 子动作。
        #[command(subcommand)]
        action: CookiesAction,
    },
}

/// `cookies` 子动作。
#[derive(Debug, Subcommand)]
enum CookiesAction {
    /// 列出某个会话的 cookie（值一律脱敏）。
    List {
        /// 会话名。
        #[arg(long, default_value = "default", value_name = "NAME")]
        session: String,
    },
    /// 导出某个会话的 cookie 到文件（需 `--yes`；文件权限 0600）。
    Export {
        /// 会话名。
        #[arg(long, default_value = "default", value_name = "NAME")]
        session: String,
        /// 目标文件；必须是相对 state dir 的相对路径（绝对路径与越界一律拒绝）。
        #[arg(long, value_name = "PATH")]
        file: PathBuf,
        /// 明确确认写入凭据。
        #[arg(long)]
        yes: bool,
    },
}

/// 进程入口：解析参数并执行；解析失败给出可读提示与非零退出码（CLI-08）。
pub fn run() -> ExitCode {
    let matches = match command().try_get_matches() {
        Ok(matches) => matches,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(clap_exit_code(&error));
        }
    };
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(2);
        }
    };
    execute(cli)
}

/// 带动态长帮助的 clap 命令（帮助里列出全部子命令、环境变量与退出码）。
fn command() -> clap::Command {
    Cli::command().after_long_help(help_footer())
}

/// CLI-07 / CLI-11：--help 的可机械断言尾部。
fn help_footer() -> String {
    let env_vars: Vec<String> = ENV_VARS.iter().map(|key| format!("  {key}")).collect();
    let exit_codes: Vec<String> = ErrorCode::ALL
        .iter()
        .map(|code| format!("  {} → {}", code.as_str(), exit_code(*code)))
        .collect();
    format!(
        "环境变量（优先级低于 CLI 参数、高于配置文件）：\n{}\n\
         只有 WEBCTL_PROXY 会启用代理；宿主 HTTP_PROXY / HTTPS_PROXY / ALL_PROXY 一律忽略，\n\
         未配置时固定直连。\n\n\
         退出码（错误码 → 进程退出码，稳定映射）：\n{}\n\n\
         配置文件为 TOML，可用键：output / timeout_ms / offline / proxy。\n\n\
         安全默认严格：cookies 导出需 --yes 且文件权限 0600；\n\
         日志与信封中的 cookie / Authorization 一律脱敏；文件参数须落在 state dir 内。",
        env_vars.join("\n"),
        exit_codes.join("\n")
    )
}

fn clap_exit_code(error: &clap::Error) -> u8 {
    u8::try_from(error.exit_code()).unwrap_or(2)
}

fn execute(cli: Cli) -> ExitCode {
    let started = Instant::now();
    let output_hint = output_hint(&cli);

    // 配置层 1/3：配置文件（可选）。
    let explicit = cli.config.clone();
    let path = explicit
        .clone()
        .unwrap_or_else(|| default_config_path(|key| std::env::var(key).ok()));
    let file_result: Result<ConfigLayer, ConfigError> = if path.exists() {
        load_config_file(&path)
    } else if explicit.is_some() {
        // 显式指定的文件不存在必须报错，不得静默回落。
        Err(ConfigError::Read {
            path: path.clone(),
            source: "文件不存在".to_owned(),
        })
    } else {
        Ok(ConfigLayer::default())
    };
    let file_layer = match file_result {
        Ok(layer) => layer,
        Err(error) => return fail(error.code(), &error.to_string(), output_hint, started),
    };

    // 配置层 2/3：环境变量。
    let env_layer = match ConfigLayer::from_env(|key| std::env::var(key).ok()) {
        Ok(layer) => layer,
        Err(error) => return fail(error.code(), &error.to_string(), output_hint, started),
    };

    // 配置层 3/3：CLI 参数，优先级最高。
    let config = resolve(&file_layer, &env_layer, &cli.to_layer());

    match cli.command {
        Command::Caps => run_caps(&config, started),
        Command::Config => run_config(&config, started),
        Command::Fetch { url } => run_fetch(&config, &url, started),
        Command::Cookies { action } => run_cookies(&config, action, started),
    }
}

/// 配置装载失败前也要尽量尊重用户选择的输出格式（CLI-04）。
fn output_hint(cli: &Cli) -> OutputFormat {
    if cli.json {
        return OutputFormat::Json;
    }
    if let Some(output) = cli.output {
        return output.into();
    }
    match std::env::var(webctl_core::config::ENV_OUTPUT) {
        Ok(value) if value.trim().eq_ignore_ascii_case("text") => OutputFormat::Text,
        _ => OutputFormat::Json,
    }
}

fn run_caps(config: &Config, started: Instant) -> ExitCode {
    let report = CapsReport::detect(config);
    // caps 是本地自检数据，因此 trust 为 trusted_local（页面内容才用 untrusted）。
    let envelope = Envelope::ok(report, Trust::TrustedLocal, elapsed(started));
    emit(envelope, config.output, render_caps)
}

fn run_config(config: &Config, started: Instant) -> ExitCode {
    let redacted = config.redacted();
    let envelope = Envelope::ok(redacted, Trust::TrustedLocal, elapsed(started));
    emit(envelope, config.output, render_config)
}

fn run_fetch(config: &Config, url: &str, started: Instant) -> ExitCode {
    // CLI-06：offline 下不发起任何连接。
    if config.offline {
        return fail(
            ErrorCode::Blocked,
            "offline 模式已启用：拒绝任何网络访问",
            config.output,
            started,
        );
    }
    // SEC-07：诊断日志里的 URL 必须先脱敏；非 http(s) 不记录（不把本地路径写进日志）。
    if webctl_core::security::paths::is_http_scheme(url) {
        crate::logs::log("fetch", url);
    }
    let options = FetchOptions {
        timeout: Duration::from_millis(config.timeout_ms),
        proxy: config.proxy.clone(),
        ..FetchOptions::default()
    };
    let envelope = match block_on(fetch(url, &options)) {
        Ok(mut envelope) => {
            redact_sensitive(&mut envelope, config.proxy.as_deref());
            envelope
        }
        Err(error) => Envelope::fail(
            ErrorCode::Internal,
            format!("运行时时初始化失败：{error}"),
            elapsed(started),
        ),
    };
    emit(envelope, config.output, render_fetch)
}

/// 用当前线程的 tokio 运行时执行一个 future（CLI 进程内单次使用）。
fn block_on<F: Future>(future: F) -> Result<F::Output, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    Ok(runtime.block_on(future))
}

/// 输出前脱敏（纵深防御：transport 已在 `finish` 脱敏；这里兜底代理凭据与未来新路径）。
fn redact_sensitive(envelope: &mut Envelope<Fetched>, proxy: Option<&str>) {
    // SEC-07：错误信息里可能带出含 token 的目标 URL（wreq 的 uri 回显），也必须脱敏。
    envelope.error = webctl_core::security::redact::redact_log_line(&envelope.error);
    if let Some(proxy) = proxy {
        envelope.error = redact_proxy(&envelope.error, proxy);
    }
    if let Some(data) = &mut envelope.data {
        webctl_web::redact::redact_fetched(data);
        // CLI-10 契约：敏感头整体置为占位符（不再保留 cookie 名）。
        for (name, value) in &mut data.headers {
            if webctl_core::security::redact::is_sensitive_header(name) {
                *value = webctl_core::security::redact::REDACTED.to_owned();
            }
        }
    }
}

/// `cookies` 清单里的一条（值恒为脱敏占位符）。
#[derive(Debug, Serialize)]
struct CookieView {
    name: String,
    value: String,
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    expires_at: Option<u64>,
}

/// `cookies list` 的信封数据。
#[derive(Debug, Serialize)]
struct CookieReport {
    session: String,
    cookies: Vec<CookieView>,
}

/// `cookies export` 的信封数据（不含任何 cookie 值）。
#[derive(Debug, Serialize)]
struct CookieExport {
    path: String,
    bytes: usize,
    mode: Option<u32>,
}

fn run_cookies(config: &Config, action: CookiesAction, started: Instant) -> ExitCode {
    match action {
        CookiesAction::List { session } => {
            let state = match load_session_for_cookies(config, &session) {
                Ok(state) => state,
                Err((code, message)) => return fail(code, &message, config.output, started),
            };
            let mut redacted = state;
            webctl_web::redact::redact_session_state(&mut redacted);
            let report = CookieReport {
                session: redacted.name.clone(),
                cookies: redacted
                    .cookies
                    .iter()
                    .map(|cookie| CookieView {
                        name: cookie.name.clone(),
                        value: cookie.value.clone(),
                        domain: cookie.domain.clone(),
                        path: cookie.path.clone(),
                        secure: cookie.secure,
                        http_only: cookie.http_only,
                        expires_at: cookie.expires_at,
                    })
                    .collect(),
            };
            emit(
                Envelope::ok(report, Trust::TrustedLocal, elapsed(started)),
                config.output,
                render_cookies,
            )
        }
        CookiesAction::Export { session, file, yes } => {
            // SEC-02：导出凭据必须先显式确认；未确认时在**任何文件/会话动作之前**拒绝。
            if !yes {
                return fail(
                    ErrorCode::Blocked,
                    "cookies export requires --yes: refusing to write credentials without explicit confirmation",
                    config.output,
                    started,
                );
            }
            // SEC-11：文件参数先过 canonicalize containment（绝对路径与越界一律拒绝）。
            let root = SafeRoot::new(config.state_dir.clone());
            let path = match root.resolve(&file) {
                Ok(path) => path,
                Err(error) => return fail(error.code(), &error.message(), config.output, started),
            };
            let state = match load_session_for_cookies(config, &session) {
                Ok(state) => state,
                Err((code, message)) => return fail(code, &message, config.output, started),
            };
            let json = match state.export_json() {
                Ok(json) => json,
                Err(error) => return fail(error.code(), &error.message(), config.output, started),
            };
            // SEC-03：写入即 0600。
            if let Err(error) = write_secret_file(&path, json.as_bytes()) {
                return fail(
                    ErrorCode::Internal,
                    &format!("导出 cookie 失败：{error}"),
                    config.output,
                    started,
                );
            }
            let export = CookieExport {
                path: path.display().to_string(),
                bytes: json.len(),
                mode: webctl_core::state::file_mode(&path),
            };
            emit(
                Envelope::ok(export, Trust::TrustedLocal, elapsed(started)),
                config.output,
                render_cookie_export,
            )
        }
    }
}

fn load_session_for_cookies(
    config: &Config,
    session: &str,
) -> Result<webctl_web::SessionState, (ErrorCode, String)> {
    let store = SessionStore::new(config.state_dir.clone());
    store
        .load(session)
        .map_err(|error| (error.code(), error.message()))
}

fn render_cookies(report: &CookieReport) {
    println!("session = {}", report.session);
    for cookie in &report.cookies {
        println!(
            "  {}@{}{} = {}（secure={} http_only={}）",
            cookie.name, cookie.domain, cookie.path, cookie.value, cookie.secure, cookie.http_only
        );
    }
}

fn render_cookie_export(export: &CookieExport) {
    println!(
        "exported {} bytes to {}（mode {}）",
        export.bytes,
        export.path,
        export
            .mode
            .map(|mode| format!("{mode:o}"))
            .unwrap_or_else(|| "n/a".to_owned())
    );
}

fn redact_proxy(message: &str, proxy: &str) -> String {
    let redacted = redact_url_userinfo(proxy);
    if redacted == proxy {
        message.to_owned()
    } else {
        message.replace(proxy, &redacted)
    }
}

/// 统一输出：JSON 单行（CLI-09）；text 成功给人类可读、失败给可读错误。
fn emit<T, R>(envelope: Envelope<T>, format: OutputFormat, render: R) -> ExitCode
where
    T: serde::Serialize,
    R: FnOnce(&T),
{
    if let Err(violation) = envelope.validate() {
        eprintln!("webctl: 内部信封不变量被破坏：{violation:?}");
        return ExitCode::from(exit_code(ErrorCode::Internal));
    }
    let code = envelope.code;
    if format == OutputFormat::Json {
        match serde_json::to_string(&envelope) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("webctl: 序列化失败：{error}");
                return ExitCode::from(exit_code(ErrorCode::Internal));
            }
        }
    } else if envelope.ok {
        if let Some(data) = &envelope.data {
            render(data);
        }
    } else {
        eprintln!("webctl: {}（{}）", envelope.error, envelope.code.as_str());
    }
    ExitCode::from(exit_code(code))
}

fn fail(code: ErrorCode, message: &str, format: OutputFormat, started: Instant) -> ExitCode {
    let envelope: Envelope<()> = Envelope::fail(code, message.to_owned(), elapsed(started));
    emit(envelope, format, |_| {})
}

fn render_caps(report: &CapsReport) {
    println!("webctl {}（engine={}）", report.version, report.engine);
    for capability in &report.capabilities {
        let state = match capability.state {
            CapabilityState::Available => "available",
            CapabilityState::Disabled => "disabled",
        };
        println!(
            "  {:<10} {:<10} {}",
            capability.domain.as_str(),
            state,
            capability.reason
        );
    }
}

fn render_config(config: &Config) {
    println!("state_dir = {}", config.state_dir.display());
    println!("output = {}", config.output.as_str());
    println!("timeout_ms = {}", config.timeout_ms);
    println!("offline = {}", config.offline);
    println!(
        "max_concurrency_per_host = {}",
        config.max_concurrency_per_host
    );
    println!("min_interval_ms = {}", config.min_interval_ms);
    println!("proxy = {}", config.proxy.as_deref().unwrap_or("(direct)"));
}

fn render_fetch(fetched: &Fetched) {
    println!(
        "HTTP {} {}（{} 字节，编码 {}）",
        fetched.status,
        fetched.final_url,
        fetched.content.byte_len,
        fetched.content.charset.as_deref().unwrap_or("binary")
    );
    if fetched.content.truncated {
        println!("  警告：响应体已按上限截断");
    }
}

fn elapsed(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CLI-05：错误码 → 退出码稳定且两两不同。
    #[test]
    fn exit_code_mapping_is_stable() {
        assert_eq!(exit_code(ErrorCode::Ok), 0);
        assert_eq!(exit_code(ErrorCode::Internal), 1);
        assert_eq!(exit_code(ErrorCode::BadUrl), 2);
        assert_eq!(exit_code(ErrorCode::Timeout), 3);
        assert_eq!(exit_code(ErrorCode::Network), 4);
        assert_eq!(exit_code(ErrorCode::Blocked), 5);
        assert_eq!(exit_code(ErrorCode::Parse), 6);
        assert_eq!(exit_code(ErrorCode::Degraded), 7);
        assert_eq!(exit_code(ErrorCode::Unsupported), 8);
        assert_eq!(exit_code(ErrorCode::SessionLost), 9);
        assert_eq!(exit_code(ErrorCode::Limit), 10);
        let mut codes: Vec<u8> = ErrorCode::ALL.iter().map(|code| exit_code(*code)).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), ErrorCode::ALL.len(), "退出码必须两两不同");
    }

    /// CLI-07 / CLI-11：帮助页列出全部子命令、环境变量与错误码。
    #[test]
    fn help_footer_documents_env_vars_and_codes() {
        let footer = help_footer();
        for key in ENV_VARS {
            assert!(footer.contains(key), "帮助缺少环境变量 {key}");
        }
        for code in ErrorCode::ALL {
            assert!(
                footer.contains(code.as_str()),
                "帮助缺少错误码 {}",
                code.as_str()
            );
        }
    }

    /// CLI-04：--json 覆盖 --output；未给出时不进 CLI 层。
    #[test]
    fn json_flag_wins_over_output() {
        let cli =
            Cli::try_parse_from(["webctl", "--output", "text", "--json", "caps"]).expect("parse");
        assert_eq!(cli.to_layer().output, Some(OutputFormat::Json));
        let cli = Cli::try_parse_from(["webctl", "--output", "text", "caps"]).expect("parse");
        assert_eq!(cli.to_layer().output, Some(OutputFormat::Text));
        let cli = Cli::try_parse_from(["webctl", "caps"]).expect("parse");
        assert_eq!(cli.to_layer().output, None);
        assert_eq!(cli.to_layer().offline, None);
    }

    /// CLI-07：全部子命令都在命令树上。
    #[test]
    fn command_tree_has_all_subcommands() {
        let command = command();
        let names: Vec<&str> = command
            .get_subcommands()
            .map(clap::Command::get_name)
            .collect();
        for expected in ["caps", "fetch", "config", "cookies"] {
            assert!(
                names.contains(&expected),
                "缺少子命令 {expected}: {names:?}"
            );
        }
    }
}
