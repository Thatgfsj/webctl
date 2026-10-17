//! rmcp ServerHandler 实现与 stdio 启动（MCP-01…MCP-11）。
//!
//! 关键契约：
//! - tools/list 恰好返回 16 个工具（注册表在 tools.rs）；
//! - tools/call 的失败走“结果信封”（CallToolResult::success 包住 ok=false 的信封），
//!   只有未注册的工具名才映射成 JSON-RPC method not found；
//! - 单次调用有显式预算（默认 60s，MCP-09），超时也以信封返回；
//! - 结果超过内联上限时落盘并返回 saved_to（MCP-11）。

use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{
    CallToolRequestMethod, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
    Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::Value;
use webctl_core::{Envelope, ErrorCode};

use crate::context::McpContext;
use crate::dispatch;
use crate::instructions::INSTRUCTIONS;
use crate::spill;
use crate::tools;

/// 单次工具调用的默认预算（毫秒）：DSH 的工具调用超时是 60s（PLAN §3.6）。
pub const DEFAULT_CALL_BUDGET_MS: u64 = 60_000;

/// webctl MCP server。
#[derive(Debug, Clone)]
pub struct WebctlMcp {
    context: Arc<McpContext>,
    call_budget: Duration,
}

impl WebctlMcp {
    /// 用上下文构造（预算默认 60s）。
    pub fn new(context: McpContext) -> Self {
        Self {
            context: Arc::new(context),
            call_budget: Duration::from_millis(DEFAULT_CALL_BUDGET_MS),
        }
    }

    /// 自定义调用预算（测试用；生产保持 60s）。
    pub fn with_call_budget(mut self, budget: Duration) -> Self {
        self.call_budget = budget;
        self
    }

    /// 上下文。
    pub fn context(&self) -> &McpContext {
        &self.context
    }

    /// 调用预算。
    pub fn call_budget(&self) -> Duration {
        self.call_budget
    }

    /// initialize 响应的静态部分（协议版本由 rmcp 协商后覆盖）。
    pub fn info() -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                tools::SERVER_NAME,
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }

    /// 执行一次工具调用（含预算）；未知工具返回 None。
    ///
    /// 与 `call_tool` 共用同一条路径，便于直接测试预算与信封语义（MCP-09/MCP-10）。
    pub async fn invoke(&self, name: &str, args: Value) -> Option<Value> {
        if !dispatch::is_known_tool(name) {
            return None;
        }
        let future = async {
            dispatch::dispatch(&self.context, name, args)
                .await
                .unwrap_or(Value::Null)
        };
        Some(self.guard(future).await)
    }

    /// 给任意 future 套上单次调用预算；超时返回 TIMEOUT 信封（MCP-09）。
    pub async fn guard<F>(&self, future: F) -> Value
    where
        F: std::future::Future<Output = Value>,
    {
        match tokio::time::timeout(self.call_budget, future).await {
            Ok(value) => value,
            Err(_) => timeout_envelope(self.call_budget),
        }
    }
}

fn timeout_envelope(budget: Duration) -> Value {
    let message = format!(
        "工具调用超过 {} ms 预算（MCP-09）：请用分片参数缩小单次工作量",
        budget.as_millis()
    );
    serde_json::to_value(Envelope::<()>::fail(ErrorCode::Timeout, message, 0))
        .unwrap_or(Value::Null)
}

impl ServerHandler for WebctlMcp {
    fn get_info(&self) -> ServerConfig {
        Self::info()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tools::tool_definitions()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name.to_string();
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);
        let Some(envelope) = self.invoke(&name, args).await else {
            return Err(McpError::method_not_found::<CallToolRequestMethod>());
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(render(
            &self.context,
            &name,
            &envelope,
        ))])
        .into())
    }
}

/// 把信封渲染成给模型的文本：超过内联上限则落盘（MCP-11）。
fn render(context: &McpContext, tool: &str, envelope: &Value) -> String {
    let Ok(rendered) = serde_json::to_string(envelope) else {
        return "{\"ok\":false,\"code\":\"INTERNAL\",\"error\":\"信封渲染失败\"}".to_owned();
    };
    match spill::ingest(context.state_dir(), tool, &rendered) {
        Ok(value) => serde_json::to_string(&value).unwrap_or_else(|_| rendered.clone()),
        Err(message) => format!(
            "{{\"ok\":false,\"data\":null,\"error\":\"大结果落盘失败：{message}\",\"code\":\"INTERNAL\"}}"
        ),
    }
}

/// 以 stdio 跑 MCP server，直到对端关闭。
pub async fn run_stdio(context: McpContext) -> Result<(), String> {
    let server = WebctlMcp::new(context);
    let running = rmcp::serve_server(server, rmcp::transport::io::stdio())
        .await
        .map_err(|error| format!("MCP initialize 失败：{error}"))?;
    running
        .waiting()
        .await
        .map_err(|error| format!("MCP 服务异常退出：{error}"))?;
    Ok(())
}
