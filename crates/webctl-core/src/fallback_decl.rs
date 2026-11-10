//! 宿主外部回退工具声明（议题 #225，路线图 0.2.3 §3.2）。
//!
//! 宿主可以用环境变量 `WEBCTL_FALLBACK_TOOL` 声明"本环境里有一个外部回退工具"，
//! 格式固定为 `<名字>:<工具名>`，例如 `playwright:mcp__playwright__browser_navigate`。
//!
//! 三条硬边界：
//!
//! 1. **零依赖**：只读一个环境变量；不探测 PATH、不起子进程、不联网、不读宿主工具列表。
//! 2. **纯建议**：webctl **绝不调用**声明的工具。声明只会出现在 `caps` 的
//!    `fallback_capability` 与降级信封 `meta.actionable.next_steps` 的 `use <tool>` 里，
//!    是否真的调用由 agent 决定。
//! 3. **失败关闭**：变量未设置、格式非法一律当作"未声明"（返回 `None`），不 panic、不猜。
//!
//! 环境读取与解析分开：`resolve` 是注入取值的纯函数（测试不改进程环境——edition 2024
//! 下 `set_var` 属 unsafe，而本 workspace 禁止 unsafe），`from_env` 才读真实进程环境。

use serde::{Deserialize, Serialize};

use crate::envelope::FallbackHint;

/// 环境变量名：宿主声明的外部回退工具。
pub const ENV_FALLBACK_TOOL: &str = "WEBCTL_FALLBACK_TOOL";

/// 宿主声明：可读名字 + 外部工具名。
///
/// 序列化形状就是 `caps` 的 `fallback_capability`：`{"name": ..., "tool": ...}`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FallbackDecl {
    /// 宿主给出的可读名字（例如 `playwright`）。
    pub name: String,
    /// 外部工具名（例如 `mcp__playwright__browser_navigate`）。
    pub tool: String,
}

impl FallbackDecl {
    /// 追加进 `next_steps` 的那一条：**给 agent 的指令，不是 webctl 的调用**。
    pub fn next_step(&self) -> String {
        format!("use {}", self.tool)
    }
}

/// 解析 `<名字>:<工具名>`；格式非法返回 `None`。
///
/// 合法要求：恰好一个冒号分段、两段都非空、各自只含可见 ASCII（无空白 / 控制字符）。
pub fn parse(raw: &str) -> Option<FallbackDecl> {
    let (name, tool) = raw.split_once(':')?;
    let name = name.trim();
    let tool = tool.trim();
    if !valid_segment(name) || !valid_segment(tool) {
        return None;
    }
    Some(FallbackDecl {
        name: name.to_owned(),
        tool: tool.to_owned(),
    })
}

/// 单段名：非空、无空白/控制字符、无第二个冒号（否则格式有歧义）。
fn valid_segment(segment: &str) -> bool {
    !segment.is_empty() && !segment.contains(':') && segment.chars().all(|ch| ch.is_ascii_graphic())
}

/// 注入取值的纯函数：`None`（未设置）或非法格式都返回 `None`。
pub fn resolve(raw: Option<&str>) -> Option<FallbackDecl> {
    raw.and_then(parse)
}

/// 读取真实进程环境；变量未设置或格式非法都返回 `None`。
///
/// 与配置层一致：同时尝试全大写与小写变量名（`WEBCTL_FALLBACK_TOOL` /
/// `webctl_fallback_tool`），都不设置才算未声明。
pub fn from_env() -> Option<FallbackDecl> {
    let raw = std::env::var(ENV_FALLBACK_TOOL)
        .ok()
        .or_else(|| std::env::var(ENV_FALLBACK_TOOL.to_ascii_lowercase()).ok());
    resolve(raw.as_deref())
}

/// 声明存在且 `playwright_suitable` 为真时，给出建议的一步；否则 `None`。
///
/// 这是 #225 的唯一判定点：**未声明时绝不产生任何外部工具名**。
pub fn next_step_for(hint: &FallbackHint, decl: Option<&FallbackDecl>) -> Option<String> {
    let decl = decl?;
    hint.playwright_suitable.then(|| decl.next_step())
}

/// 把 `use <tool>` 追加进 `next_steps`（已存在则不重复），返回是否追加。
pub fn push_next_step(
    next_steps: &mut Vec<String>,
    hint: &FallbackHint,
    decl: Option<&FallbackDecl>,
) -> bool {
    let Some(step) = next_step_for(hint, decl) else {
        return false;
    };
    if next_steps.contains(&step) {
        return false;
    }
    next_steps.push(step);
    true
}
