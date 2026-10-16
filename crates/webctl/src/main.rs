//! webctl CLI 入口（PLAN v0.4 §3.7）。
//!
//! PR 01 只提供 caps：内置能力自检。其余子命令随各 PR 落地。
//! 硬约束：本进程不启动任何子进程（PLAN v0.4 §3.1）。
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand, ValueEnum};
use webctl_core::{CapabilityState, CapsReport, Config, Envelope, ErrorCode, OutputFormat, Trust};

/// 错误码到进程退出码的稳定映射。
fn exit_code(code: ErrorCode) -> u8 {
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

/// CLI 输出格式。
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
    about = "给 AI agent 用的网页控制层：全部内置，零外部进程、零浏览器"
)]
struct Cli {
    /// 输出格式。
    #[arg(long, global = true, value_enum, default_value_t = OutputArg::Json)]
    output: OutputArg,

    /// 等价于 --output json（默认；与 PLAN/测试用例里的 caps --json 兼容）。
    #[arg(long, global = true)]
    json: bool,

    /// 离线模式：不允许任何网络访问。
    #[arg(long, global = true)]
    offline: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 内置能力自检（不联网、不探测外部程序）。
    Caps,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let output = if cli.json {
        OutputFormat::Json
    } else {
        cli.output.into()
    };
    let config = Config {
        output,
        offline: cli.offline,
        ..Config::default()
    };

    let started = Instant::now();
    match cli.command {
        Command::Caps => run_caps(&config, started),
    }
}

fn run_caps(config: &Config, started: Instant) -> ExitCode {
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let report = CapsReport::detect(config);
    // caps 是本地自检数据，因此 trust 为 trusted_local（页面内容才用 untrusted）。
    let envelope = Envelope::ok(report, Trust::TrustedLocal, elapsed_ms);
    if let Err(violation) = envelope.validate() {
        eprintln!("webctl: 内部信封不变量被破坏：{violation:?}");
        return ExitCode::from(exit_code(ErrorCode::Internal));
    }

    if matches!(config.output, OutputFormat::Text) {
        if let Some(report) = &envelope.data {
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
        return ExitCode::SUCCESS;
    }

    match serde_json::to_string_pretty(&envelope) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("webctl: 序列化失败：{error}");
            ExitCode::from(exit_code(ErrorCode::Internal))
        }
    }
}
