//! 内置能力自检（caps）。
//!
//! 硬约束（PLAN v0.4 §3.1）：caps 不联网、不探测外部程序、不启动子进程。
//! 因此这里只报告进程内能力域与**实际注册**的工具，不出现任何外部工具名。
//!
//! # 诚实性（议题 #68）
//!
//! `interface` 域只有在调用方真的注册了工具时才可能为 available；
//! `detect_with` 的 `tools` 参数必须来自接口层的真实注册表（webctl-mcp 的派发表）。
//! 工具“存在但不可用”不得被报告成 available——这是发布说明与 README 的唯一事实来源。
use serde::{Deserialize, Serialize};

use crate::config::Config;

/// 能力域（PLAN v0.4 §2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityDomain {
    /// 取回：HTTP(S)、重定向链、cookie jar、编码、内容类型路由。
    Fetch,
    /// 解析：DOM、正文、链接、表格、表单、markdown、JSON/XML。
    Parse,
    /// 会话：cookie 持久化、表单提交、CSRF、multipart、分页。
    Session,
    /// 文档与数据：PDF 文本层、JSON 查询、格式转换。
    DocData,
    /// 接口：CLI 与 MCP。
    Interface,
}

impl CapabilityDomain {
    /// 全部能力域，顺序固定。
    pub const ALL: [CapabilityDomain; 5] = [
        Self::Fetch,
        Self::Parse,
        Self::Session,
        Self::DocData,
        Self::Interface,
    ];

    /// 线上字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fetch => "fetch",
            Self::Parse => "parse",
            Self::Session => "session",
            Self::DocData => "doc_data",
            Self::Interface => "interface",
        }
    }

    /// 面向用户的说明。
    pub fn description(self) -> &'static str {
        match self {
            Self::Fetch => "取回：HTTP(S)、重定向链、cookie jar、编码、内容类型路由",
            Self::Parse => "解析：DOM、正文、链接、表格、表单、markdown、JSON/XML",
            Self::Session => "会话：cookie 持久化、表单提交、CSRF、multipart、分页",
            Self::DocData => "文档与数据：PDF 文本层、JSON 查询、格式转换",
            Self::Interface => "接口：CLI 与 MCP",
        }
    }
}

/// 能力状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    /// 可用。
    Available,
    /// 被配置显式关闭。
    Disabled,
    /// 不存在或未注册（缺失）。
    Unavailable,
}

/// 单个能力域的报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    /// 能力域。
    pub domain: CapabilityDomain,
    /// 状态。
    pub state: CapabilityState,
    /// 原因说明（人类可读）。
    pub reason: String,
}

/// 单个工具的能力报告（§3.7 的 18 个工具逐项）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCapability {
    /// 原始工具名（不带 mcp__webctl__ 前缀）。
    pub name: String,
    /// 状态；缺失必须是 Unavailable，绝不允许 available。
    pub state: CapabilityState,
    /// 原因说明（人类可读）。
    pub reason: String,
}

impl ToolCapability {
    /// 该工具是否可用。
    pub fn is_available(&self) -> bool {
        self.state == CapabilityState::Available
    }
}

/// 内置 JS 引擎的能力状态（议题 #128 / [ADR-0005](../../docs/adr/0005-embedded-js-engine.md)）。
///
/// `enabled` 与 `wired` **必须分开**：前者是调用方是否显式开启，后者是引擎是否真的接上了
/// 取回/抽取路径。分开是为了消灭"配置说开着、实际没执行"这种假象。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsCapability {
    /// 调用方是否显式开启（默认关闭）。
    pub enabled: bool,
    /// 引擎是否**已接线**（能真正执行页面脚本）。
    pub wired: bool,
    /// 诚实说明（不得为空，也不必与 `enabled` 一致）。
    pub reason: String,
}

impl Default for JsCapability {
    fn default() -> Self {
        Self {
            enabled: false,
            wired: false,
            // 默认值**不声称**任何接线状态：接线情况由接口层按实际填充（见 mcp/caps.rs）。
            reason: "未提供引擎状态（由接口层按实际接线情况填充）".to_owned(),
        }
    }
}

/// SPA / JS 页面的支持面（#115）。
///
/// 两部分都是**机器可读**的：`detected_containers` 是引擎认得的容器类型，
/// `channels` 是建议调用方在遇到 JS 页时尝试的通道（**工具名**）。
/// `channels` 必须只包含真实注册的工具——由 MCP 层的过滤与测试保证。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaSurface {
    /// 认得的内嵌数据容器类型。
    pub detected_containers: Vec<String>,
    /// 可尝试的通道（工具名）。
    pub channels: Vec<String>,
}

/// 能力自检报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapsReport {
    /// webctl 版本。
    pub version: String,
    /// 执行引擎，固定 builtin。
    pub engine: &'static str,
    /// 能力域清单。
    pub capabilities: Vec<Capability>,
    /// 工具清单（由接口层实际注册情况派生）。
    pub tools: Vec<ToolCapability>,
    /// SPA / JS 页面的支持面（#115）。
    pub spa: SpaSurface,
    /// 内置 JS 引擎状态（#128）。
    pub js: JsCapability,
}

impl CapsReport {
    /// 没有接口层工具时的报告（core 单元测试 / CLI-only 场景）。
    pub fn detect(config: &Config) -> Self {
        Self::detect_with(config, Vec::new())
    }

    /// 按配置与实际注册的工具生成报告。
    ///
    /// `tools` 必须来自接口层的真实注册表；空列表意味着“没有接口工具”，
    /// 此时 interface 为 unavailable，而不是硬编码 available。
    pub fn detect_with(config: &Config, tools: Vec<ToolCapability>) -> Self {
        let interface_available = tools.iter().any(ToolCapability::is_available);
        let capabilities = CapabilityDomain::ALL
            .into_iter()
            .map(|domain| {
                let (state, reason) = match domain {
                    CapabilityDomain::Fetch if config.offline => (
                        CapabilityState::Disabled,
                        format!("{}：离线模式已关闭", domain.description()),
                    ),
                    CapabilityDomain::Interface if !interface_available => (
                        CapabilityState::Unavailable,
                        "MCP/接口工具未注册（本进程没有可用的接口层工具）".to_owned(),
                    ),
                    _ => (
                        CapabilityState::Available,
                        format!("{}（进程内实现，无外部依赖）", domain.description()),
                    ),
                };
                Capability {
                    domain,
                    state,
                    reason,
                }
            })
            .collect();
        Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            engine: "builtin",
            capabilities,
            tools,
            spa: SpaSurface::default(),
            js: JsCapability::default(),
        }
    }

    /// 附加 SPA 支持面（#115）：由接口层用真实容器类型与真实工具名填充。
    pub fn with_spa(mut self, spa: SpaSurface) -> Self {
        self.spa = spa;
        self
    }

    /// 附加内置 JS 引擎状态（#128）：由接口层按**实际接线情况**填充。
    pub fn with_js(mut self, js: JsCapability) -> Self {
        self.js = js;
        self
    }

    /// 取某个能力域。
    pub fn get(&self, domain: CapabilityDomain) -> &Capability {
        self.capabilities
            .iter()
            .find(|capability| capability.domain == domain)
            .expect("报告必定包含全部能力域")
    }

    /// 该能力域是否可用。
    pub fn is_available(&self, domain: CapabilityDomain) -> bool {
        self.get(domain).state == CapabilityState::Available
    }

    /// 取某个工具的报告（若有）。
    pub fn tool(&self, name: &str) -> Option<&ToolCapability> {
        self.tools.iter().find(|tool| tool.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stub_tool(name: &str) -> ToolCapability {
        ToolCapability {
            name: name.to_owned(),
            state: CapabilityState::Available,
            reason: "stub".to_owned(),
        }
    }

    /// #128：`js` 的 `enabled` 与 `wired` 必须**分开**报告；默认值不声称接线状态。
    #[test]
    fn js_capability_separates_enabled_from_wired() {
        let default_report = CapsReport::detect(&Config::default());
        assert!(!default_report.js.enabled, "默认必须关闭");
        assert!(
            !default_report.js.wired,
            "默认值不得声称已接线（接线状态由接口层填）"
        );
        assert!(!default_report.js.reason.is_empty(), "原因不得为空");
        let enabled_config = Config {
            js_render: true,
            ..Config::default()
        };
        let report = CapsReport::detect(&enabled_config).with_js(JsCapability {
            enabled: true,
            wired: false,
            reason: "已开启；接线状态由接口层填".to_owned(),
        });
        assert!(report.js.enabled);
        assert!(
            !report.js.wired,
            "开启配置 ≠ 已接线：不得把两者混为一谈（否则就是假象）"
        );
    }

    /// CAPS-01：五个能力域齐全且顺序固定。
    #[test]
    fn reports_all_domains_in_order() {
        let report = CapsReport::detect(&Config::default());
        assert_eq!(report.capabilities.len(), 5);
        let domains: Vec<&str> = report
            .capabilities
            .iter()
            .map(|c| c.domain.as_str())
            .collect();
        assert_eq!(
            domains,
            ["fetch", "parse", "session", "doc_data", "interface"]
        );
        for domain in [
            CapabilityDomain::Fetch,
            CapabilityDomain::Parse,
            CapabilityDomain::Session,
            CapabilityDomain::DocData,
        ] {
            assert!(report.is_available(domain));
        }
        assert_eq!(report.engine, "builtin");
        assert!(!report.version.is_empty());
    }

    /// CAPS-02：报告里不得出现任何外部工具或浏览器（v0.4 已全部内置）。
    #[test]
    fn report_never_mentions_external_tools() {
        let report = CapsReport::detect_with(
            &Config::default(),
            vec![stub_tool("fetch"), stub_tool("caps")],
        );
        let json = serde_json::to_string(&report)
            .expect("serialize")
            .to_lowercase();
        for forbidden in [
            "pdftotext",
            "tesseract",
            "pandoc",
            "jq",
            "chrome",
            "chromium",
            "edge",
            "playwright",
            "cdp",
        ] {
            assert!(
                !json.contains(forbidden),
                "caps 报告不应包含外部依赖名：{forbidden}"
            );
        }
    }

    /// CAPS-08 的前置：离线配置只影响取回域。
    #[test]
    fn offline_disables_fetch_only() {
        let config = Config {
            offline: true,
            ..Config::default()
        };
        let report = CapsReport::detect_with(&config, vec![stub_tool("fetch")]);
        assert!(!report.is_available(CapabilityDomain::Fetch));
        assert!(report.is_available(CapabilityDomain::Parse));
        assert!(report.is_available(CapabilityDomain::Session));
        assert!(report.is_available(CapabilityDomain::DocData));
        assert!(report.is_available(CapabilityDomain::Interface));
        assert!(report.get(CapabilityDomain::Fetch).reason.contains("离线"));
    }

    /// 议题 #68：没有注册任何工具时 interface 不得报 available。
    #[test]
    fn missing_tools_never_report_interface_available() {
        let report = CapsReport::detect(&Config::default());
        assert_eq!(
            report.get(CapabilityDomain::Interface).state,
            CapabilityState::Unavailable
        );
        assert!(report.tools.is_empty());
    }

    /// 议题 #68：存在但不可用的工具不得被洗成 available，也不能点亮 interface。
    #[test]
    fn unavailable_tools_do_not_light_up_interface() {
        let tool = ToolCapability {
            name: "fetch".to_owned(),
            state: CapabilityState::Unavailable,
            reason: "未注册".to_owned(),
        };
        let report = CapsReport::detect_with(&Config::default(), vec![tool]);
        assert!(!report.tool("fetch").expect("present").is_available());
        assert_eq!(
            report.get(CapabilityDomain::Interface).state,
            CapabilityState::Unavailable
        );
    }

    /// 议题 #68：真注册且可用的工具才点亮 interface。
    #[test]
    fn registered_tools_light_up_interface() {
        let report = CapsReport::detect_with(&Config::default(), vec![stub_tool("fetch")]);
        assert_eq!(
            report.get(CapabilityDomain::Interface).state,
            CapabilityState::Available
        );
        assert!(report.tool("fetch").expect("present").is_available());
    }
}
