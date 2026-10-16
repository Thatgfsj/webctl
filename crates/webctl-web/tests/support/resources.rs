//! 资源观测工具（议题 #22 §5；服务 RES-03,05…10、SEC-10、MCP-14、ENV-12、CAPS-06）。
//!
//! 目标是让"资源占用/并发/子进程"这类说法变成可断言的**数字**：
//! - fd 计数：/proc/self/fd；
//! - RSS：/proc/self/status 的 VmRSS（直接以 kB 计，不猜页大小）；
//! - 子进程计数：扫描 /proc 里 PPid 指向本进程的条目；
//! - CPU 空转采样：/proc/self/stat 的 utime+stime（USER_HZ 假定 100）。
//!
//! 硬约束：
//! - 零子进程：本文件只用 std::fs 读 /proc，不创建任何进程；
//! - 零 unsafe：不直接做系统调用；
//! - 可移植性：只有 Linux 提供真实数字，其余平台返回 None，
//!   断言侧遇到 None 会显式跳过而不是假装通过（见 tests/resource_observation.rs）。
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 当前平台是否支持真实资源采样（目前仅 Linux）。
pub const PLATFORM_SUPPORTED: bool = cfg!(target_os = "linux");

/// Linux 用户态时钟频率假定值；cpu_ticks 换算成秒时使用。
pub const USER_HZ: u64 = 100;

#[cfg(target_os = "linux")]
mod platform {
    /// 读 /proc/self 符号链接拿到本进程 pid（避免调用子进程 API）。
    fn own_pid() -> Option<u32> {
        let target = std::fs::read_link("/proc/self").ok()?;
        target.file_name()?.to_str()?.parse().ok()
    }

    /// 打开着的文件描述符数量。
    pub fn fd_count() -> Option<usize> {
        Some(std::fs::read_dir("/proc/self/fd").ok()?.count())
    }

    /// 常驻内存集（RSS），字节。
    pub fn rss_bytes() -> Option<u64> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kb * 1024);
            }
        }
        None
    }

    /// 进程累计 utime + stime 时钟节拍数。
    pub fn cpu_ticks() -> Option<u64> {
        let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
        ticks(&stat)
    }

    /// 当前线程累计 utime + stime（并行测试下不会被其他测试线程污染）。
    pub fn thread_cpu_ticks() -> Option<u64> {
        let stat = std::fs::read_to_string("/proc/thread-self/stat").ok()?;
        ticks(&stat)
    }

    fn ticks(stat: &str) -> Option<u64> {
        let after = stat.rsplit_once(')')?.1;
        let fields: Vec<&str> = after.split_whitespace().collect();
        // after 的分段：0=state(field3) … 11=utime(field14)、12=stime(field15)。
        let utime: u64 = fields.get(11)?.parse().ok()?;
        let stime: u64 = fields.get(12)?.parse().ok()?;
        Some(utime + stime)
    }

    /// 本进程当前直接子进程数量。
    pub fn child_process_count() -> Option<usize> {
        let pid = own_pid()?;
        let mut count = 0;
        for entry in std::fs::read_dir("/proc").ok()?.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.chars().all(|ch| ch.is_ascii_digit()) {
                continue;
            }
            if read_ppid(&entry.path().join("stat")) == Some(pid) {
                count += 1;
            }
        }
        Some(count)
    }

    fn read_ppid(stat_path: &std::path::Path) -> Option<u32> {
        let stat = std::fs::read_to_string(stat_path).ok()?;
        let after = stat.rsplit_once(')')?.1;
        after.split_whitespace().nth(1)?.parse().ok()
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    pub fn fd_count() -> Option<usize> {
        None
    }
    pub fn rss_bytes() -> Option<u64> {
        None
    }
    pub fn cpu_ticks() -> Option<u64> {
        None
    }
    pub fn thread_cpu_ticks() -> Option<u64> {
        None
    }
    pub fn child_process_count() -> Option<usize> {
        None
    }
}

/// 当前打开的文件描述符数（Linux 为实数，其余平台 None）。
pub fn fd_count() -> Option<usize> {
    platform::fd_count()
}

/// 当前常驻内存集（字节；其余平台 None）。
pub fn rss_bytes() -> Option<u64> {
    platform::rss_bytes()
}

/// 当前进程累计 CPU 时钟节拍（utime + stime）。
pub fn cpu_ticks() -> Option<u64> {
    platform::cpu_ticks()
}

/// 当前直接子进程数（用于 A8 / RES-10 / SEC-10 / MCP-14 / CAPS-06）。
pub fn child_process_count() -> Option<usize> {
    platform::child_process_count()
}

/// 在 window 时间内采样**整个进程**的 CPU 占用百分比。
///
/// 并行测试下会被其他测试线程抬高，RES-08 请改用 thread_cpu_percent_over。
/// 该值用 USER_HZ 假定换算，精度只够做"是否忙等"的粗断言，不做性能测量。
pub fn cpu_percent_over(window: Duration) -> Option<f64> {
    percent_over(window, cpu_ticks)
}

/// 在 window 时间内采样**当前线程**的 CPU 占用百分比（空闲时应接近 0）。
///
/// 线程级采样不受 cargo 并行测试影响，因此 RES-08 的可断言数字用它。
pub fn thread_cpu_percent_over(window: Duration) -> Option<f64> {
    percent_over(window, thread_cpu_ticks)
}

/// 当前线程累计 CPU 时钟节拍。
pub fn thread_cpu_ticks() -> Option<u64> {
    platform::thread_cpu_ticks()
}

fn percent_over(window: Duration, ticks: fn() -> Option<u64>) -> Option<f64> {
    let before = ticks()?;
    let started = Instant::now();
    std::thread::sleep(window);
    let after = ticks()?;
    let wall = started.elapsed().as_secs_f64();
    if wall <= 0.0 {
        return None;
    }
    Some((after.saturating_sub(before) as f64 / USER_HZ as f64) / wall * 100.0)
}

/// 多次采样的 RSS 记录器。
#[derive(Debug, Default, Clone)]
pub struct RssSampler {
    samples: Vec<u64>,
}

impl RssSampler {
    /// 新建空采样器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 采一次样，返回本次读数。
    pub fn sample(&mut self) -> Option<u64> {
        let value = rss_bytes()?;
        self.samples.push(value);
        Some(value)
    }

    /// 全部采样值。
    pub fn samples(&self) -> &[u64] {
        &self.samples
    }

    /// 首次采样。
    pub fn first(&self) -> Option<u64> {
        self.samples.first().copied()
    }

    /// 末次采样。
    pub fn last(&self) -> Option<u64> {
        self.samples.last().copied()
    }

    /// 峰值采样。
    pub fn peak(&self) -> Option<u64> {
        self.samples.iter().copied().max()
    }

    /// 末次减首次（正数表示增长）。
    pub fn growth(&self) -> Option<i64> {
        Some(self.last()? as i64 - self.first()? as i64)
    }
}

/// 仓库根目录（由 crate 目录上推一级）。
pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate 目录应有上级 workspace 根")
        .to_path_buf()
}

/// 递归收集 root 下的 .rs 文件（跳过 target/.git/node_modules）。
pub fn rust_sources_under(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect(root, &mut files);
    files.sort();
    files
}

fn collect(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(name.as_ref(), "target" | ".git" | "node_modules") {
                continue;
            }
            collect(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// 一次"子进程调用点"命中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubprocessCallSite {
    /// 文件。
    pub path: PathBuf,
    /// 行号（1 起）。
    pub line: usize,
    /// 该行内容（去尾空白）。
    pub text: String,
}

/// 扫描 root 下 .rs 文件里**真正会创建子进程**的 API。
///
/// 只匹配 process::Command / Command::new( / .spawn(；
/// 不匹配合法的 ExitCode 枚举或 clap 的 Command 类型。
pub fn subprocess_call_sites(root: &Path) -> Vec<SubprocessCallSite> {
    // 把匹配串拆开拼装，避免本文件自身被扫描命中。
    let needles = [
        ["process", "Command"].join("::"),
        ["Command", "new("].join("::"),
        [".spawn", "("].join(""),
    ];
    let mut sites = Vec::new();
    for file in rust_sources_under(root) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if needles.iter().any(|needle| line.contains(needle)) {
                sites.push(SubprocessCallSite {
                    path: file.clone(),
                    line: index + 1,
                    text: line.trim().to_owned(),
                });
            }
        }
    }
    sites
}
