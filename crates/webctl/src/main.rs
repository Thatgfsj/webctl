//! webctl CLI 入口（PLAN v0.4 §3.7）。
//!
//! 只做进程入口：参数解析、配置装载与执行在 cli 模块。
//! 硬约束：本进程不启动任何子进程（PLAN v0.4 §3.1）。
use std::process::ExitCode;

mod cli;

fn main() -> ExitCode {
    cli::run()
}
