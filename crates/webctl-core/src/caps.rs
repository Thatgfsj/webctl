//! 内置能力自检（caps）。
//!
//! 硬约束（PLAN v0.4 §3.1）：caps 不联网、不探测外部程序、不启动子进程。
//! 因此这里只报告进程内能力域的状态，不出现任何外部工具名。
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

/// 能力自检报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapsReport {
    /// webctl 版本。
    pub version: String,
    /// 执行引擎，固定 builtin。
    pub engine: &'static str,
    /// 能力域清单。
    pub capabilities: Vec<Capability>,
}

impl CapsReport {
    /// 按配置生成报告。离线只影响取回域。
    pub fn detect(config: &Config) -> Self {
        let capabilities = CapabilityDomain::ALL
            .into_iter()
            .map(|domain| {
                let available = !(config.offline && domain == CapabilityDomain::Fetch);
                Capability {
                    domain,
                    state: if available {
                        CapabilityState::Available
                    } else {
                        CapabilityState::Disabled
                    },
                    reason: if available {
                        format!("{}（进程内实现，无外部依赖）", domain.description())
                    } else {
                        format!("{}：离线模式已关闭", domain.description())
                    },
                }
            })
            .collect();
        Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            engine: "builtin",
            capabilities,
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for domain in CapabilityDomain::ALL {
            assert!(report.is_available(domain));
        }
        assert_eq!(report.engine, "builtin");
        assert!(!report.version.is_empty());
    }

    /// CAPS-02：报告里不得出现任何外部工具或浏览器（v0.4 已全部内置）。
    #[test]
    fn report_never_mentions_external_tools() {
        let report = CapsReport::detect(&Config::default());
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
        let report = CapsReport::detect(&config);
        assert!(!report.is_available(CapabilityDomain::Fetch));
        assert!(report.is_available(CapabilityDomain::Parse));
        assert!(report.is_available(CapabilityDomain::Session));
        assert!(report.is_available(CapabilityDomain::DocData));
        assert!(report.is_available(CapabilityDomain::Interface));
        assert!(report.get(CapabilityDomain::Fetch).reason.contains("离线"));
    }
}
