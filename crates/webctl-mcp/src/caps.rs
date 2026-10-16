//! MCP 层的 caps 视图：把“实际注册的工具”翻译成 core 的 ToolCapability（议题 #68）。
//!
//! 唯一事实来源是 `crate::dispatch::is_known_tool`——也就是工具真的在派发表里；
//! 不是把 15 个名字硬编码成 available。

use webctl_core::{CapabilityState, ToolCapability};

use crate::dispatch;
use crate::tools::TOOL_SPECS;

/// 逐项报告 15 个工具的可用性（顺序与 PLAN §3.7 一致）。
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
        assert_eq!(capabilities.len(), 15);
        for capability in &capabilities {
            assert!(capability.is_available(), "{} 应已注册", capability.name);
        }
    }

    #[test]
    fn capabilities_mirror_the_dispatch_table() {
        for capability in tool_capabilities() {
            assert_eq!(capability.is_available(), dispatch::is_known_tool(&capability.name));
        }
        assert!(!dispatch::is_known_tool("screenshot"));
    }
}
