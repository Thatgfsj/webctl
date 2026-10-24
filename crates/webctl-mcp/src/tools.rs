//! 工具注册表：17 个工具的名字、说明与 inputSchema（PLAN v0.4 §3.7 / MCP-02…04）。
//!
//! 名字是**契约**：与 PLAN §3.7 逐字一致，且 DSH 挂载后形如 mcp__webctl__<rawName>。
//! 工具的 inputSchema 由 schemars 从 params.rs 的分支类型派生，保证合法 JSON Schema。

use rmcp::model::Tool;

use crate::params;

/// MCP server 名（DSH 工具名 mcp__<serverName>__<rawName> 的中间段）。
pub const SERVER_NAME: &str = "webctl";

/// 17 个工具的名字，顺序与 PLAN v0.4 §3.7 表格一致。
pub const TOOL_NAMES: [&str; 17] = [
    "fetch",
    "extract",
    "search",
    "links",
    "tables",
    "forms",
    "fill_form",
    "paginate",
    "session",
    "cookies",
    "json_query",
    "doc_text",
    "embedded_data",
    "download",
    "robots",
    "resolve_site",
    "caps",
];

/// 单个工具的静态元信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolSpec {
    /// 原始工具名（未加 mcp__webctl__ 前缀）。
    pub name: &'static str,
    /// 面向模型的一句话说明。
    pub description: &'static str,
}

impl ToolSpec {
    /// DSH 挂载后的限定名。
    pub fn qualified(&self) -> String {
        qualified_name(self.name)
    }
}

/// 把原始工具名转成 DSH 契约名 mcp__webctl__<rawName>。
pub fn qualified_name(raw: &str) -> String {
    format!("mcp__{SERVER_NAME}__{raw}")
}

/// 17 个工具的静态清单（顺序同 TOOL_NAMES）。
pub const TOOL_SPECS: [ToolSpec; 17] = [
    ToolSpec {
        name: "fetch",
        description: "取回 http(s) URL：状态、头、重定向链、内容或落盘路径",
    },
    ToolSpec {
        name: "extract",
        description: "用 CSS 选择器抽取元素，返回结构化命中；零命中是合法空结果",
    },
    ToolSpec {
        name: "search",
        description: "页面内文本/正则搜索，返回命中位置与上下文",
    },
    ToolSpec {
        name: "links",
        description: "链接清单：绝对化与 rel/域名/正则过滤",
    },
    ToolSpec {
        name: "tables",
        description: "表格结构化：行列与跨行跨列语义确定",
    },
    ToolSpec {
        name: "forms",
        description: "表单与字段清单：类型、默认值、action/method",
    },
    ToolSpec {
        name: "fill_form",
        description: "一次调用填表并提交（含隐藏字段与 CSRF）",
    },
    ToolSpec {
        name: "paginate",
        description: "分页遍历：next 识别与页数上限",
    },
    ToolSpec {
        name: "session",
        description: "会话查看/保存/加载/清理（cookie 持久化）",
    },
    ToolSpec {
        name: "cookies",
        description: "查看/导出 cookie（导出需 yes=true，值脱敏）",
    },
    ToolSpec {
        name: "json_query",
        description: "内置 JSON 查询子集（替代 jq，零外部进程）",
    },
    ToolSpec {
        name: "doc_text",
        description: "PDF/HTML/XML 文本层提取（替代 pdftotext/pandoc）",
    },
    ToolSpec {
        name: "embedded_data",
        description: "抽取页面内嵌的结构化数据（JSON-LD / script[type=json] / 框架状态容器 / data-page）",
    },
    ToolSpec {
        name: "download",
        description: "文件下载到指定目录（尺寸上限与落盘路径）",
    },
    ToolSpec {
        name: "robots",
        description: "robots.txt 与站点策略查询（礼貌爬取依据）",
    },
    ToolSpec {
        name: "resolve_site",
        description: "按站点名给出入口候选（L1 内置映射/域名直推 + L3 Wikidata P856 + L4 sitemap 声明；L5 兜底需显式 allow_serp_fallback=true；多候选、带证据与置信度，不单选）",
    },
    ToolSpec {
        name: "caps",
        description: "内置能力与版本自检，逐项列出 17 个工具的可用性",
    },
];

/// 由 schemars 生成 inputSchema 的 rmcp 工具定义。
fn tool<P: schemars::JsonSchema + 'static>(name: &'static str, description: &'static str) -> Tool {
    Tool::new(name, description, serde_json::Map::new()).with_input_schema::<P>()
}

/// 供 tools/list 使用的 17 个工具定义（顺序与 TOOL_NAMES 一致）。
pub fn tool_definitions() -> Vec<Tool> {
    let [
        fetch,
        extract,
        search,
        links,
        tables,
        forms,
        fill_form,
        paginate,
        session,
        cookies,
        json_query,
        doc_text,
        embedded_data,
        download,
        robots,
        resolve_site,
        caps,
    ] = TOOL_NAMES;
    vec![
        tool::<params::FetchParams>(fetch, TOOL_SPECS[0].description),
        tool::<params::ExtractParams>(extract, TOOL_SPECS[1].description),
        tool::<params::SearchParams>(search, TOOL_SPECS[2].description),
        tool::<params::LinksParams>(links, TOOL_SPECS[3].description),
        tool::<params::TablesParams>(tables, TOOL_SPECS[4].description),
        tool::<params::FormsParams>(forms, TOOL_SPECS[5].description),
        tool::<params::FillFormParams>(fill_form, TOOL_SPECS[6].description),
        tool::<params::PaginateParams>(paginate, TOOL_SPECS[7].description),
        tool::<params::SessionParams>(session, TOOL_SPECS[8].description),
        tool::<params::CookiesParams>(cookies, TOOL_SPECS[9].description),
        tool::<params::JsonQueryParams>(json_query, TOOL_SPECS[10].description),
        tool::<params::DocTextParams>(doc_text, TOOL_SPECS[11].description),
        tool::<params::EmbeddedDataParams>(embedded_data, TOOL_SPECS[12].description),
        tool::<params::DownloadParams>(download, TOOL_SPECS[13].description),
        tool::<params::RobotsParams>(robots, TOOL_SPECS[14].description),
        tool::<params::ResolveSiteParams>(resolve_site, TOOL_SPECS[15].description),
        tool::<params::CapsParams>(caps, TOOL_SPECS[16].description),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_exactly_the_plan_list() {
        assert_eq!(TOOL_SPECS.len(), TOOL_NAMES.len());
        for (index, spec) in TOOL_SPECS.iter().enumerate() {
            assert_eq!(spec.name, TOOL_NAMES[index]);
        }
    }

    #[test]
    fn definitions_match_the_static_specs() {
        let definitions = tool_definitions();
        assert_eq!(definitions.len(), TOOL_SPECS.len());
        for (definition, spec) in definitions.iter().zip(TOOL_SPECS.iter()) {
            assert_eq!(definition.name, spec.name);
            assert_eq!(definition.description.as_deref(), Some(spec.description));
        }
    }

    #[test]
    fn qualified_names_follow_the_dsh_contract() {
        for spec in TOOL_SPECS {
            assert_eq!(spec.qualified(), format!("mcp__webctl__{}", spec.name));
        }
    }
}
