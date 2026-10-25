//! 17 个工具的 MCP 输入参数（schemars 生成 inputSchema，MCP-03）。
//!
//! 字段名与 PLAN v0.4 §3.7 的工具语义一一对应；可选字段用 Option，
//! 缺省值在派发层解释。这里只描述形状，不做业务校验。

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;

/// fetch：取回 URL。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FetchParams {
    /// 目标 http(s) URL。
    pub url: String,
    /// 响应体字节上限；超过会截断并返回 LIMIT 降级。
    pub max_bytes: Option<usize>,
    /// 是否跟随重定向（默认 true）。
    pub follow_redirects: Option<bool>,
    /// 单次请求超时（毫秒）。
    pub timeout_ms: Option<u64>,
}

/// extract：按 CSS 选择器抽取（url 或 html 二选一）。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ExtractParams {
    /// CSS 选择器。
    pub selector: String,
    /// 目标 URL；与 html 二选一。
    pub url: Option<String>,
    /// 内存中的 HTML；与 url 二选一。
    pub html: Option<String>,
    /// 最多返回的命中数。
    pub max_results: Option<usize>,
    /// 返回文本的字符预算。
    pub char_budget: Option<usize>,
    /// 每条命中保留的上下文两侧字符数。
    pub context_chars: Option<usize>,
}

/// search：页面内文本/正则搜索。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// 查询串。
    pub query: String,
    /// 目标 URL；与 html 二选一。
    pub url: Option<String>,
    /// 内存中的文本/HTML；与 url 二选一。
    pub html: Option<String>,
    /// 是否把 query 当作正则（默认 false）。
    pub regex: Option<bool>,
    /// 是否大小写不敏感（默认 false）。
    pub case_insensitive: Option<bool>,
    /// 最多返回的命中数。
    pub max_results: Option<usize>,
    /// 命中两侧保留的上下文字符数。
    pub context_chars: Option<usize>,
}

/// links：链接清单与过滤。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LinksParams {
    /// 目标 URL；与 html 二选一。
    pub url: Option<String>,
    /// 内存中的 HTML；与 url 二选一。
    pub html: Option<String>,
    /// 解析相对链接用的基准 URL。
    pub base: Option<String>,
    /// 只保留这些域名的链接。
    pub domains: Option<Vec<String>>,
    /// 只保留带这些 rel 的链接。
    pub rel: Option<Vec<String>>,
    /// 链接 URL 必须匹配的正则。
    pub pattern: Option<String>,
    /// 只要内部链接。
    pub internal_only: Option<bool>,
    /// 只要外部链接。
    pub external_only: Option<bool>,
    /// 最多返回的链接数。
    pub max_results: Option<usize>,
}

/// tables：表格结构化。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TablesParams {
    /// 目标 URL；与 html 二选一。
    pub url: Option<String>,
    /// 内存中的 HTML；与 url 二选一。
    pub html: Option<String>,
    /// 最多返回的表格数。
    pub max_results: Option<usize>,
}

/// forms：表单与字段清单。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FormsParams {
    /// 目标 URL；与 html 二选一。
    pub url: Option<String>,
    /// 内存中的 HTML；与 url 二选一。
    pub html: Option<String>,
    /// 解析 action 相对路径用的基准 URL。
    pub base: Option<String>,
    /// 只返回该序号的表单（从 0 起）。
    pub form_index: Option<usize>,
    /// 最多返回的表单数。
    pub max_results: Option<usize>,
}

/// fill_form：一次调用填表并提交。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FillFormParams {
    /// 包含表单的页面 URL（会先取回该页）。
    pub url: String,
    /// 表单序号（从 0 起，默认 0）。
    pub form_index: Option<usize>,
    /// 字段名到值的映射；复选框/单选用 check/select 表述。
    pub fields: BTreeMap<String, String>,
    /// 复选框勾选状态（名 -> 是否勾选）。
    pub checks: Option<BTreeMap<String, bool>>,
    /// 会话名（须与绑定会话一致）。
    pub session: Option<String>,
}

/// paginate：分页遍历。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PaginateParams {
    /// 起始 URL。
    pub url: String,
    /// 最多遍历的页数（默认由引擎决定）。
    pub max_pages: Option<usize>,
    /// 会话名（须与绑定会话一致）。
    pub session: Option<String>,
}

/// session 动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionAction {
    /// 列出已保存的会话。
    List,
    /// 查看指定会话（cookie 值脱敏）。
    Show,
    /// 保存（新建/覆盖）指定会话。
    Save,
    /// 加载指定会话。
    Load,
    /// 清理指定会话。
    Clear,
}

/// session：会话查看/保存/加载/清理。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SessionParams {
    /// 动作。
    pub action: SessionAction,
    /// 会话名（list 之外的动作必填）。
    pub name: Option<String>,
    /// 调用方声明的会话名（用于 MCP-07 会话归属校验）。
    pub session: Option<String>,
}

/// cookies 动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CookiesAction {
    /// 查看 cookie（值脱敏）。
    View,
    /// 导出 cookie 到落盘文件（需 yes=true，值脱敏）。
    Export,
}

/// cookies：查看/导出 cookie。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CookiesParams {
    /// 动作。
    pub action: CookiesAction,
    /// 限定的 URL（只返回对该 URL 可见的 cookie）。
    pub url: Option<String>,
    /// 会话名（须与绑定会话一致）。
    pub session: Option<String>,
    /// 导出确认：导出必须显式传 true。
    pub yes: Option<bool>,
}

/// json_query：内置 JSON 查询子集。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct JsonQueryParams {
    /// 查询表达式。
    pub expression: String,
    /// 内联 JSON 文本；与 url/path 三选一。
    pub json: Option<String>,
    /// 取回 JSON 的 URL；与 json/path 三选一。
    pub url: Option<String>,
    /// 本地 JSON 文件路径；与 json/url 三选一。
    pub path: Option<String>,
}

/// doc_text：PDF / HTML / XML 文本层提取。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DocTextParams {
    /// 本地文件路径；与 url 二选一。
    pub path: Option<String>,
    /// 取回文档的 URL；与 path 二选一。
    pub url: Option<String>,
}

/// embedded_data：抽取页面内嵌的结构化数据（零 JS 执行）。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EmbeddedDataParams {
    /// 目标 URL；与 html 二选一（同 `extract`：本工具不从本地文件读，本地文档走 `doc_text`）。
    pub url: Option<String>,
    /// 内存中的 HTML；与 url 二选一（离线用例用）。
    pub html: Option<String>,
    /// 最多返回的内嵌载荷条数。
    pub max_entries: Option<usize>,
    /// 单条载荷的字节上限（超过则标记 truncated 且不解析）。
    pub max_entry_bytes: Option<usize>,
    /// 是否抽取内联状态赋值（window.__NUXT__ 这类）；默认 true。
    pub include_inline_state: Option<bool>,
}

/// download：文件下载到指定目录。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DownloadParams {
    /// 目标 URL。
    pub url: String,
    /// 目标目录（默认 state dir 下的 downloads/）。
    pub dir: Option<String>,
    /// 文件名（默认从 URL 末段推断）。
    pub filename: Option<String>,
    /// 字节上限。
    pub max_bytes: Option<usize>,
}

/// robots：robots.txt 与站点策略查询。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RobotsParams {
    /// 站点内任意 URL；以其 origin 查 robots.txt。
    pub url: String,
    /// 用于匹配规则的 User-Agent（默认 webctl）。
    pub user_agent: Option<String>,
}

/// caps：内置能力与版本自检。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CapsParams {
    /// 是否附带 17 个工具逐项可用性（默认 true）。
    pub include_tools: Option<bool>,
}

/// resolve_site：按站点名定位入口候选（L1 本地；L3/L4/L5 会联网）。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ResolveSiteParams {
    /// 站点名/品牌名（如 "知乎"、"hacker news"）；**不**接受完整 URL（有 URL 就直接用 `fetch`）。
    pub site: String,
    /// 最多返回的候选数（默认 5，上限 10）。
    pub max_candidates: Option<usize>,
    /// **显式开启 L5 兜底**（抓搜索结果的 SERP 页）：默认 **false**。开启后会在 stderr 留痕；
    /// 结果按不可靠来源处理（低分、不跨过可靠阈值），被拦即如实报 BLOCKED。
    pub allow_serp_fallback: Option<bool>,
}
