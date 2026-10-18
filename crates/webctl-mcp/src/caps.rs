//! MCP 层的 caps 视图：把“实际注册的工具”翻译成 core 的 ToolCapability（议题 #68）。
//!
//! 唯一事实来源是 `crate::dispatch::is_known_tool`——也就是工具真的在派发表里；
//! 不是把 16 个名字硬编码成 available。

use webctl_core::{CapabilityState, Config, JsCapability, SpaSurface, ToolCapability};

use crate::dispatch;
use crate::tools::TOOL_SPECS;

/// 遇到 JS 页面时建议调用方尝试的**通道候选**（工具名）。
///
/// 这些名字必须真实注册：`spa_surface()` 会用 `dispatch::is_known_tool` 过滤，
/// 且测试断言候选表本身没写错（写错了就红，而不是悄悄少一个建议）。
pub const SUGGESTED_CHANNELS: [&str; 1] = ["embedded_data"];

/// 构造内置 JS 引擎状态（#128 / ADR-0005）。
///
/// **关键**：`wired` 反映"引擎是否真的接上了取回/抽取路径"，而不是"配置有没有开"。
/// 自 #128 步骤 2 起**已接线**（`resolve_source` + `extract` 的 URL 分支，条件：开关开启且内容为 HTML），
/// 故 `wired = true`；`reason` 必须写清触发条件与能力边界（只执行内联脚本、最小 DOM 子集）。
pub fn js_capability(config: &Config) -> JsCapability {
    JsCapability {
        enabled: config.js_render,
        wired: true,
        reason: "已接线到 resolve_source 与 extract 的 URL 分支：开关开启且内容为 HTML 时才渲染；\
                渲染失败不伪装成功而是降级（degraded_from=js-render-failed）。**只执行内联脚本**：外链 <script src> 不取，\
                因此依赖 jQuery 等外部库的页面会如实降级（真站实测 ReferenceError: $ is not defined）。\
                覆盖内容是**最小 DOM 子集**，复杂框架水合大概率仍取不到（boa ≠ 浏览器，见 ADR-0005）"
            .to_owned(),
    }
}

/// 构造 SPA 支持面（#115）：认得的容器类型 + 真实可用的通道。
pub fn spa_surface() -> SpaSurface {
    SpaSurface {
        detected_containers: webctl_web::parse::embedded::CONTAINER_KINDS
            .iter()
            .map(|kind| (*kind).to_owned())
            .collect(),
        channels: SUGGESTED_CHANNELS
            .iter()
            .filter(|name| dispatch::is_known_tool(name))
            .map(|name| (*name).to_owned())
            .collect(),
    }
}

/// 逐项报告 16 个工具的可用性（顺序与 PLAN §3.7 一致）。
pub fn tool_capabilities() -> Vec<ToolCapability> {
    TOOL_SPECS
        .iter()
        .map(|spec| {
            let registered = dispatch::is_known_tool(spec.name);
            ToolCapability {
                name: spec.name.to_owned(),
                state: if registered {
                    CapabilityState::Available
                } else {
                    CapabilityState::Unavailable
                },
                reason: if registered {
                    format!("{}（已注册：{}）", spec.description, spec.qualified())
                } else {
                    "未注册到 MCP 派发表".to_owned()
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_plan_tools_are_registered_and_available() {
        let capabilities = tool_capabilities();
        assert_eq!(capabilities.len(), crate::tools::TOOL_NAMES.len());
        for capability in &capabilities {
            assert!(capability.is_available(), "{} 应已注册", capability.name);
        }
    }

    #[test]
    fn capabilities_mirror_the_dispatch_table() {
        for capability in tool_capabilities() {
            assert_eq!(
                capability.is_available(),
                dispatch::is_known_tool(&capability.name)
            );
        }
        assert!(!dispatch::is_known_tool("screenshot"));
    }
}
