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

/// 仓库根目录：从本 crate 的 CARGO_MANIFEST_DIR 向上找到含 [workspace] 的 Cargo.toml。
///
/// 只在这里定位仓库根；调用点**不得**再拼接 crates（历史上
/// workspace_root().join("crates") 曾算成 <repo>/crates/crates，让扫描器恒空）。
/// 需要源码扫描根时用 crates_root()。
pub fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let manifest = dir.join("Cargo.toml");
        if let Ok(text) = std::fs::read_to_string(&manifest)
            && text
                .lines()
                .any(|line| line.trim_start().starts_with("[workspace"))
        {
            return dir;
        }
        if !dir.pop() {
            panic!(
                "无法从 {} 向上找到含 [workspace] 的 Cargo.toml",
                env!("CARGO_MANIFEST_DIR")
            );
        }
    }
}

/// 静态扫描根：仓库根下的 crates 目录。crates 只在这里拼接一次。
pub fn crates_root() -> PathBuf {
    workspace_root().join("crates")
}

/// 扫描失败：根目录不存在、不可读或条目读取失败。
///
/// **不得**在目录缺失时静默返回空集——那会把"路径写错"伪装成"没有违规"。
#[derive(Debug)]
pub struct ScanError {
    path: PathBuf,
    source: std::io::Error,
}

impl ScanError {
    fn new(path: &Path, source: std::io::Error) -> Self {
        Self {
            path: path.to_path_buf(),
            source,
        }
    }

    /// 出错的路径。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "扫描失败 {}：{}",
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for ScanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// 递归收集 root 下的 .rs 文件（跳过 target/.git/node_modules）。
///
/// root 不存在或不可读时返回 ScanError，**绝不**静默返回空集。
pub fn rust_sources_under(root: &Path) -> Result<Vec<PathBuf>, ScanError> {
    let mut files = Vec::new();
    collect(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), ScanError> {
    let entries = std::fs::read_dir(root).map_err(|error| ScanError::new(root, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| ScanError::new(root, error))?;
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(name.as_ref(), "target" | ".git" | "node_modules") {
                continue;
            }
            collect(&path, out)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// 去掉 Rust 源码里的注释，保留字符串/字符字面量原文与所有换行（行号不变）。
///
/// 处理 //、///、//! 行注释与**可嵌套**的 /* … */ 块注释。字符串字面量里的 //
/// （如 https://…）不会被误当注释截断——否则 URL 扫描会漏报或错报。
///
/// 已知边界：不解析 raw string 与单引号转义这类极端字符字面量；
/// 二者在本仓库的测试源码里不出现。
fn strip_comments(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    let mut line_comment = false;
    let mut block_depth = 0usize;
    let mut in_string = false;
    while index < chars.len() {
        let ch = chars[index];
        if line_comment {
            if ch == '\n' {
                out.push(ch);
                line_comment = false;
            }
            index += 1;
            continue;
        }
        if block_depth > 0 {
            if ch == '*' && chars.get(index + 1) == Some(&'/') {
                block_depth -= 1;
                index += 2;
                continue;
            }
            if ch == '/' && chars.get(index + 1) == Some(&'*') {
                block_depth += 1;
                index += 2;
                continue;
            }
            if ch == '\n' {
                out.push(ch);
            }
            index += 1;
            continue;
        }
        if in_string {
            out.push(ch);
            if ch == '\\' {
                if let Some(&escaped) = chars.get(index + 1) {
                    out.push(escaped);
                    index += 2;
                    continue;
                }
            } else if ch == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if ch == '/' && chars.get(index + 1) == Some(&'/') {
            line_comment = true;
            index += 2;
            continue;
        }
        if ch == '/' && chars.get(index + 1) == Some(&'*') {
            block_depth = 1;
            index += 2;
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            index += 1;
            continue;
        }
        if ch == '\'' && is_char_literal(&chars, index) {
            out.push(ch);
            index += 1;
            while index < chars.len() {
                let ch = chars[index];
                out.push(ch);
                index += 1;
                if ch == '\\' {
                    if let Some(&escaped) = chars.get(index) {
                        out.push(escaped);
                        index += 1;
                    }
                } else if ch == '\'' {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
        index += 1;
    }
    out
}

/// chars[index] 是单引号时，判断它开启字符字面量还是生命周期（如 'a）。
fn is_char_literal(chars: &[char], index: usize) -> bool {
    match chars.get(index + 1) {
        Some('\\') => chars.get(index + 3) == Some(&'\''),
        Some(_) => chars.get(index + 2) == Some(&'\''),
        None => false,
    }
}

/// 扫描器自身文件（相对任意扫描根的后缀）：按定义排除，避免自命中。
///
/// 该排除不削弱检测能力——scanners_detect_planted_violations 用临时文件
/// 自证"植入违规必然被检出"。
const SCANNER_SELF_SUFFIX: &str = "tests/support/resources.rs";

fn is_scanner_self(path: &Path) -> bool {
    path.ends_with(SCANNER_SELF_SUFFIX)
}

/// 只把 …/tests/… 下的 .rs 当作"测试源码"。
fn is_test_source(path: &Path) -> bool {
    path.to_string_lossy()
        .replace('\\', "/")
        .contains("/tests/")
}

/// 一次"真实外站 URL"命中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalUrlSite {
    /// 文件。
    pub path: PathBuf,
    /// 行号（1 起）。
    pub line: usize,
    /// 命中的 URL（scheme + authority）。
    pub url: String,
    /// 该行内容（去尾空白）。
    pub text: String,
}

/// 扫描测试源码（路径含 /tests/ 的 .rs）里的**真实外站 URL 字面量**。
///
/// 规则（权威实现；与 tests/golden/README.md 同步）：
/// 1. 跳过注释（见 strip_comments）与扫描器自身文件；
/// 2. http:// 与 https:// 同等对待，不再"见 https 即违规"；
/// 3. authority（scheme 后到分隔符之间）去掉 {…} 格式占位符后**为空** →
///    模板主机，如 format!("http://{addr}/")、format!("http://{}", addr)，放行；
/// 4. authority 为回环（127.0.0.0/8、localhost、::1）→ 放行；
/// 5. authority 属于 RFC 6761/2606 保留域名（.test、.invalid、.example、
///    .localhost、example.com/net/org）→ 放行：规范上不可解析，
///    只会出现在 Url::parse/cookie 构造等纯本地逻辑里；
/// 6. 其余 authority → 判为外站字面量。
pub fn external_url_sites(root: &Path) -> Result<Vec<ExternalUrlSite>, ScanError> {
    let scheme_http = ["http", "://"].join("");
    let scheme_https = ["https", "://"].join("");
    let schemes = [scheme_http.as_str(), scheme_https.as_str()];
    let mut sites = Vec::new();
    for file in rust_sources_under(root)? {
        if is_scanner_self(&file) || !is_test_source(&file) {
            continue;
        }
        let text = std::fs::read_to_string(&file).map_err(|error| ScanError::new(&file, error))?;
        let code = strip_comments(&text);
        for (index, line) in code.lines().enumerate() {
            // scheme 大小写不敏感：`HTTPS://` 同样要能被检出，不能成为绕过手段。
            let lowercase = line.to_ascii_lowercase();
            for scheme in schemes {
                let mut from = 0;
                while let Some(offset) = lowercase[from..].find(scheme) {
                    let start = from + offset;
                    let url_start = start + scheme.len();
                    let rest = &line[url_start..];
                    let end = authority_end(rest);
                    let host = strip_userinfo(&rest[..end]);
                    if !is_allowed_authority(host) {
                        sites.push(ExternalUrlSite {
                            path: file.clone(),
                            line: index + 1,
                            url: format!("{scheme}{host}"),
                            text: line.trim().to_owned(),
                        });
                    }
                    from = url_start;
                }
            }
        }
    }
    Ok(sites)
}

/// authority 的结束位置（字节偏移）。
///
/// 两处必须处理，否则会把**本地**用法误判成外站：
/// 1. `user:pass@host` 形式的 userinfo —— 其中的 `:` 不是 authority 分隔符
///    （否则 `http://user:hunter2@127.0.0.1:1` 会被截成 `user`）；
/// 2. `[::1]` 这类 IPv6 字面量 —— 要整体取出，不能在内层 `:` 处截断。
fn authority_end(rest: &str) -> usize {
    let mut start = 0;
    if let Some(at) = rest.find('@') {
        // 只有当 '@' 之前没有路径/引号类分隔符时，才认定它是 userinfo 的结尾。
        if !rest[..at].contains(is_path_delimiter) {
            start = at + 1;
        }
    }
    let tail = &rest[start..];
    let end = if tail.starts_with('[') {
        tail.find(']').map(|close| close + 1).unwrap_or(tail.len())
    } else {
        tail.find(is_authority_delimiter).unwrap_or(tail.len())
    };
    start + end
}

/// 路径/引号类分隔符：出现在 `@` 之前就说明那个 `@` 不是 userinfo 的分隔符。
fn is_path_delimiter(ch: char) -> bool {
    matches!(ch, '/' | '"' | '\'' | ')' | ' ' | ',' | ';' | '}' | '>')
}

/// 去掉 authority 里的 userinfo，只留 host（含端口）。
fn strip_userinfo(authority: &str) -> &str {
    authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority)
}

/// authority 的结束分隔符。
fn is_authority_delimiter(ch: char) -> bool {
    matches!(
        ch,
        '/' | '"' | '\'' | ')' | ' ' | ':' | ',' | ';' | '}' | '>'
    )
}

/// 判断 authority 是否属于"本地/保留"用法（放行）而非真实外站。
fn is_allowed_authority(authority: &str) -> bool {
    // 去掉 format! 占位符 {…}（可能嵌套）。
    let mut stripped = String::with_capacity(authority.len());
    let mut depth = 0usize;
    for ch in authority.chars() {
        match ch {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => stripped.push(ch),
            _ => {}
        }
    }
    let host = stripped.trim().trim_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return true; // 纯模板主机
    }
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    if host == "::1" || host == "[::1]" {
        return true;
    }
    if host.starts_with("127.") {
        return true;
    }
    const RESERVED_TLDS: [&str; 4] = ["test", "invalid", "example", "localhost"];
    if RESERVED_TLDS
        .iter()
        .any(|tld| host == *tld || host.ends_with(&format!(".{tld}")))
    {
        return true;
    }
    const RESERVED_DOMAINS: [&str; 3] = ["example.com", "example.net", "example.org"];
    RESERVED_DOMAINS
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
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
/// 匹配 process::Command / Command::new( / .spawn(（保守：宁可多报，人工判定）；
/// 不匹配 clap 的 Command 类型或 ExitCode。扫描前剥离注释，
/// 避免文档注释里的字面量自命中。根目录缺失时返回 Err。
pub fn subprocess_call_sites(root: &Path) -> Result<Vec<SubprocessCallSite>, ScanError> {
    // 把匹配串拆开拼装，避免本文件自身被扫描命中。
    // 只匹配**真正的调用点**：不再把 `use std::process::Command;` 这类导入当作违规
    // （导入本身不创建进程；把它算违规会在 CLI 集成测试里产生误报）。
    let needles = [["Command", "new("].join("::"), [".spawn", "("].join("")];
    // 集成测试启动**本仓自己的二进制**（Cargo 注入的 CARGO_BIN_EXE_<name>）不是"外部进程"，
    // 这正是 CLI 端到端测试的标准做法。标记拆开拼装，避免本文件自命中。
    let own_binary = ["CARGO", "BIN", "EXE_"].join("_");
    let mut sites = Vec::new();
    for file in rust_sources_under(root)? {
        if is_scanner_self(&file) {
            continue;
        }
        let text = std::fs::read_to_string(&file).map_err(|error| ScanError::new(&file, error))?;
        let code = strip_comments(&text);
        let lines: Vec<&str> = code.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if !needles.iter().any(|needle| line.contains(needle)) {
                continue;
            }
            // 允许调用点自身或紧邻的前两行出现该标记，
            // 以覆盖 `let bin = env!("CARGO_BIN_EXE_webctl"); Command::new(bin)` 这种写法。
            let from = index.saturating_sub(2);
            if lines[from..=index]
                .iter()
                .any(|near| near.contains(&own_binary))
            {
                continue;
            }
            sites.push(SubprocessCallSite {
                path: file.clone(),
                line: index + 1,
                text: line.trim().to_owned(),
            });
        }
    }
    Ok(sites)
}
