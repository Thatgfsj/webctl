//! 工具派发：把 18 个工具接到既有能力（MCP-10/MCP-11/MCP-13/SEC-09）。
//!
//! 约定：
//! - 已知工具的**任何**执行失败都以结果信封返回（ok=false + code + 可读 error），
//!   而不是 JSON-RPC 协议错误；只有“未注册的工具名”在 server 层映射成 method not found。
//! - 页面/网络内容一律 meta.trust=untrusted_page_content；只有 caps 这种本地自检用 trusted_local。
//! - 会话名先过归属校验（MCP-07），再落到与 CLI 共享的 state dir（MCP-13）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use url::Url;
use webctl_core::{EnginePath, Envelope, ErrorCode, JsRenderMode, Trust};
use webctl_web::degrade::Assessment;
use webctl_web::doc::{DocOptions, doc_text, json_query};
use webctl_web::extract::{self, ExtractOptions, FieldSpec, LinkFilter, SearchOptions};
use webctl_web::politeness::{PoliteFetcher, PolitenessConfig};
use webctl_web::session::{FormFill, SessionEngine, SessionState};
use webctl_web::transport::{Fetched, fetch};

use crate::context::McpContext;
use crate::extract_tool::{MISSING_TARGET, extract_body_value, extract_value, field_specs};
use crate::params::*;
use crate::tools::TOOL_NAMES;

/// 工具名是否已注册（caps 诚实性由此派生）。
pub fn is_known_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

/// 派发一次调用；未知工具返回 None。
pub async fn dispatch(ctx: &McpContext, name: &str, args: Value) -> Option<Value> {
    let args = if args.is_null() { json!({}) } else { args };
    let value = match name {
        "fetch" => tool_fetch(ctx, &args).await,
        "extract" => tool_extract(ctx, &args).await,
        "search" => tool_search(ctx, &args).await,
        "links" => tool_links(ctx, &args).await,
        "tables" => tool_tables(ctx, &args).await,
        "forms" => tool_forms(ctx, &args).await,
        "fill_form" => tool_fill_form(ctx, &args).await,
        "paginate" => tool_paginate(ctx, &args).await,
        "session" => tool_session(ctx, &args).await,
        "cookies" => tool_cookies(ctx, &args).await,
        "json_query" => tool_json_query(ctx, &args).await,
        "doc_text" => tool_doc_text(ctx, &args).await,
        "embedded_data" => crate::embedded_tool::tool_embedded_data(ctx, &args).await,
        "web_search" => crate::web_search_tool::tool_web_search(ctx, &args).await,
        "download" => tool_download(ctx, &args).await,
        "robots" => tool_robots(ctx, &args).await,
        "resolve_site" => crate::site::tool_resolve_site(ctx, &args).await,
        "caps" => tool_caps(ctx, &args).await,
        _ => return None,
    };
    Some(value)
}

// ---------------------------------------------------------------- 基础设施

pub(crate) fn parse<P: DeserializeOwned>(args: &Value) -> Result<P, Value> {
    serde_json::from_value::<P>(args.clone())
        .map_err(|error| fail(ErrorCode::Parse, format!("工具参数非法：{error}")))
}

pub(crate) fn to_value<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value).unwrap_or_else(|error| {
        json!({
            "ok": false,
            "data": Value::Null,
            "error": format!("信封序列化失败：{error}"),
            "code": "INTERNAL",
            "meta": Value::Null
        })
    })
}

pub(crate) fn fail(code: ErrorCode, message: impl Into<String>) -> Value {
    to_value(Envelope::<()>::fail(code, message, 0))
}

fn offline_guard(ctx: &McpContext) -> Option<Value> {
    ctx.config
        .offline
        .then(|| fail(ErrorCode::Blocked, "offline 模式已启用：拒绝任何网络访问"))
}

/// SEC-09：把 PR 12 的脱敏入口显式接到 MCP 自己拿到的取回结果上。
///
/// 取回层（`transport::finish`）本身已调用 `redact_fetched`；这里再显式接一次，
/// 让 **MCP 自己**对任何进入信封 / 序列化 / 落盘的 `Fetched` 负责，而不是依赖
/// 「上游记得脱敏」。脱敏幂等：重复调用只是把占位符再脱敏一次。
fn redact_fetched_envelope(envelope: &mut Envelope<Fetched>) {
    if let Some(data) = &mut envelope.data {
        webctl_web::redact::redact_fetched(data);
    }
}

async fn fetch_target(ctx: &McpContext, url: &str) -> Envelope<Fetched> {
    let mut envelope = fetch(url, &ctx.fetch_options()).await;
    redact_fetched_envelope(&mut envelope);
    envelope
}

/// 解析 html/text 输入：url 与 html 二选一。
/// 页面 HTML 的来源结果。`render` 记录"是否尝试渲染、以及渲染失败的原因"。
pub(crate) struct Source {
    /// 供解析/抽取使用的 HTML（渲染成功时是渲染后的，失败时是原始 HTML）。
    pub(crate) html: String,
    /// 渲染失败的说明；`None` 表示没渲染或渲染成功。
    pub(crate) render: Option<RenderNote>,
    /// 这份 HTML 的生成路径（#192）：static 或 static+js。
    pub(crate) engine_path: EnginePath,
    /// 这份 HTML 的基准 URL（#116）：url 输入为重定向后的最终 URL，html 输入为 None。
    pub(crate) base_url: Option<Url>,
}

/// 渲染失败时写进信封的降级说明（A3：不许伪装成功）。
pub(crate) struct RenderNote {
    /// 降级来源标签。
    pub(crate) degraded_from: String,
    /// 机器可读原因（引擎给出的类别）。
    pub(crate) reason: String,
    /// 观察到的证据（原始错误文本）。
    pub(crate) detected: Vec<String>,
}

/// 把渲染失败的降级**如实**写进工具自己的信封：置 `DEGRADED`、写 `degraded_from`，
/// 并带上 `meta.actionable`（原因 + 证据 + 建议通道）。
///
/// 注意：**不**把原本可用的结果改成失败——原始 HTML 仍然可用，所以是降级而不是失败。
pub(crate) fn apply_render_degradation(mut envelope: Value, note: Option<&RenderNote>) -> Value {
    let Some(note) = note else {
        return envelope;
    };
    if let Some(meta) = envelope.get_mut("meta") {
        meta["degraded_from"] = json!(note.degraded_from);
        meta["actionable"] = json!({
            "reason": note.reason,
            "detected": note.detected,
            // 空数组 = 没有别的通道可试；不写不存在的通道名。
            "next_steps": Vec::<String>::new(),
        });
    }
    envelope["code"] = serde_json::to_value(ErrorCode::Degraded).unwrap_or(json!("DEGRADED"));
    envelope["error"] = json!(note.reason);
    envelope
}

use crate::degradation::{UpstreamDegrade, apply_upstream_degradation};

/// 把 HTML 生成路径写进信封（#192）。
///
/// 只在成功信封上写：失败时没有“解析用的 HTML”，写路径没有意义。
fn apply_engine_path(mut envelope: Value, path: EnginePath) -> Value {
    if envelope.get("ok").and_then(Value::as_bool) != Some(true) {
        return envelope;
    }
    if let Some(meta) = envelope.get_mut("meta") {
        meta["engine_path"] = serde_json::to_value(path).unwrap_or(Value::Null);
    }
    envelope
}

/// 一次渲染尝试（#192）。
struct RenderedPage {
    /// 供解析使用的 HTML：成功时是渲染后的 DOM，失败时回落到原始 HTML。
    html: String,
    /// 渲染失败说明；成功或未渲染时为 None。
    note: Option<RenderNote>,
    /// 是否**真的**执行了渲染。
    rendered: bool,
}

impl RenderedPage {
    /// 未渲染：原样使用静态 HTML。
    fn static_html(html: &str) -> Self {
        Self {
            html: html.to_owned(),
            note: None,
            rendered: false,
        }
    }

    /// 实际路径：只有真的渲染过才写 static+js。
    fn engine_path(&self) -> EnginePath {
        if self.rendered {
            EnginePath::StaticThenJs
        } else {
            EnginePath::Static
        }
    }
}

/// 渲染一次；失败返回原始 HTML + 降级说明（不静默）。
fn render_page(html: &str) -> RenderedPage {
    match webctl_js::render(html, &webctl_js::JsLimits::default()) {
        Ok(outcome) => RenderedPage {
            html: outcome.html,
            note: None,
            rendered: true,
        },
        Err(failure) => RenderedPage {
            html: html.to_owned(),
            note: Some(RenderNote {
                degraded_from: "js-render-failed".to_owned(),
                reason: format!("{:?}", failure.kind).to_lowercase(),
                detected: vec![failure.message.clone()],
            }),
            rendered: false,
        },
    }
}

/// `auto` 的“需要 JS”判定：只认 degrade 的结论，静态页一律放过（#192）。
fn auto_wants_render(html: &str) -> bool {
    matches!(
        webctl_web::degrade::assess(html),
        Assessment::NeedsJs { .. }
    )
}

/// 解析类工具（`resolve_source`）的渲染决策。
///
/// - `off`：不渲染；
/// - `on`：全量渲染（旧的 `true` 语义）；
/// - `auto`：先静态判定，只有 `degrade` 说需要 JS 才对**这一页**渲染一次。
fn render_for_parse(ctx: &McpContext, html: &str, is_html: bool) -> RenderedPage {
    if !is_html {
        return RenderedPage::static_html(html);
    }
    match ctx.config.js_render {
        JsRenderMode::Off => RenderedPage::static_html(html),
        JsRenderMode::On => render_page(html),
        JsRenderMode::Auto => {
            if auto_wants_render(html) {
                render_page(html)
            } else {
                RenderedPage::static_html(html)
            }
        }
    }
}

/// 用渲染结果重跑一次 `extract`（支持 `fields`），并把渲染失败说明与 engine_path 一起写进信封。
fn finish_extract_fields(
    page: RenderedPage,
    selector: Option<&str>,
    fields: Option<&BTreeMap<String, FieldSpec>>,
    options: &ExtractOptions,
) -> Value {
    let engine_path = page.engine_path();
    let envelope = apply_render_degradation(
        extract_value(&page.html, selector, fields, options),
        page.note.as_ref(),
    );
    apply_engine_path(envelope, engine_path)
}

/// 静态抽取是否“零命中”：选择器无 match 且 fields 全为 null（fields 缺失视为真）。
fn static_extract_empty(envelope: &Value) -> bool {
    let Some(data) = envelope.get("data") else {
        return false;
    };
    let matches_empty = data
        .get("matches")
        .and_then(Value::as_array)
        .is_none_or(|matches| matches.is_empty());
    let fields_empty = match data.get("fields") {
        Some(Value::Object(map)) => !map.is_empty() && map.values().all(Value::is_null),
        Some(_) => false,
        None => true,
    };
    matches_empty && fields_empty
}

/// 把渲染说明与 engine_path 一起挂到解析结果信封上（#192）。
fn finish_source(envelope: Value, source: &Source) -> Value {
    apply_engine_path(
        apply_render_degradation(envelope, source.render.as_ref()),
        source.engine_path,
    )
}

pub(crate) async fn resolve_source(
    ctx: &McpContext,
    url: Option<&str>,
    html: Option<&str>,
) -> Result<Source, Value> {
    match (url, html) {
        (Some(_), Some(_)) => Err(fail(
            ErrorCode::BadUrl,
            "url 与 html 只能给一个（两者同时给出无法判定内容来源）",
        )),
        (None, None) => Err(fail(ErrorCode::BadUrl, "必须给出 url 或 html 之一")),
        // 调用方自带 HTML：同样视为“一个页面”，由策略决定是否渲染。
        (None, Some(html)) => {
            let page = render_for_parse(ctx, html, true);
            let engine_path = page.engine_path();
            Ok(Source {
                html: page.html,
                render: page.note,
                engine_path,
                base_url: None,
            })
        }
        (Some(url), None) => {
            if let Some(rejected) = offline_guard(ctx) {
                return Err(rejected);
            }
            let envelope = fetch_target(ctx, url).await;
            if !envelope.ok {
                return Err(to_value(envelope));
            }
            let fetched = envelope.data.expect("ok envelope has data");
            let base_url = Url::parse(&fetched.final_url).ok();
            let is_html = matches!(fetched.content.kind, webctl_web::ContentKind::Html);
            match fetched.content.text {
                Some(text) => {
                    let page = render_for_parse(ctx, &text, is_html);
                    let engine_path = page.engine_path();
                    Ok(Source {
                        html: page.html,
                        render: page.note,
                        engine_path,
                        base_url,
                    })
                }
                None => Err(to_value(Envelope::degraded(
                    json!({"content_kind": format!("{:?}", fetched.content.kind).to_lowercase()}),
                    format!("content-type:{:?}", fetched.content.kind).to_lowercase(),
                    ErrorCode::Unsupported,
                    0,
                ))),
            }
        }
    }
}

// ---------------------------------------------------------------- 18 个工具

async fn tool_fetch(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<FetchParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    if let Some(rejected) = offline_guard(ctx) {
        return rejected;
    }
    let mut options = ctx.fetch_options();
    if let Some(max_bytes) = params.max_bytes {
        options.max_body_bytes = max_bytes.max(1);
    }
    if let Some(follow) = params.follow_redirects {
        options.follow_redirects = follow;
    }
    if let Some(timeout_ms) = params.timeout_ms {
        options.timeout = Duration::from_millis(timeout_ms.max(1));
    }
    let mut envelope = fetch(&params.url, &options).await;
    redact_fetched_envelope(&mut envelope);
    to_value(envelope)
}

async fn tool_extract(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<ExtractParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    let mut options = ExtractOptions::default();
    if let Some(value) = params.max_results {
        options.max_results = value;
    }
    if let Some(value) = params.char_budget {
        options.char_budget = value;
    }
    if let Some(value) = params.context_chars {
        options.context_chars = value;
    }
    // #190：续读模式必须拿到 start_index 之后的全部匹配，抽取阶段不能再按
    // char_budget 提前截断；char_budget 在窗口层作为本次返回的字符预算。
    if params.start_index.is_some() {
        options.char_budget = usize::MAX;
    }
    let selector = params.selector.as_deref();
    let fields = field_specs(&params);
    let fields = fields.as_ref();
    if selector.is_none() && fields.is_none() {
        return fail(ErrorCode::Parse, MISSING_TARGET);
    }
    let envelope = match (params.url.as_deref(), params.html.as_deref()) {
        (Some(url), None) => {
            if let Some(rejected) = offline_guard(ctx) {
                return rejected;
            }
            let envelope = fetch_target(ctx, url).await;
            if !envelope.ok {
                return to_value(envelope);
            }
            let fetched = envelope.data.expect("ok envelope has data");
            let is_html = matches!(fetched.content.kind, webctl_web::ContentKind::Html);
            // 非 HTML：逐字走原路径（内容类型路由不属于渲染范畴）。
            if !is_html {
                extract_body_value(&fetched.content, selector, fields, &options)
            } else {
                let raw = fetched.content.text.clone().unwrap_or_default();
                match ctx.config.js_render {
                    // 关闭时**逐字**走原路径（硬要求：默认行为不变）。
                    JsRenderMode::Off => apply_engine_path(
                        extract_body_value(&fetched.content, selector, fields, &options),
                        EnginePath::Static,
                    ),
                    // 全量：所有 HTML 渲染一次再解析（旧 true 语义）。
                    JsRenderMode::On => {
                        finish_extract_fields(render_page(&raw), selector, fields, &options)
                    }
                    // 自适应：先静态抽取；只有零命中且判空壳时才渲染该页。
                    JsRenderMode::Auto => {
                        let envelope =
                            extract_body_value(&fetched.content, selector, fields, &options);
                        if static_extract_empty(&envelope) && auto_wants_render(&raw) {
                            finish_extract_fields(render_page(&raw), selector, fields, &options)
                        } else {
                            apply_engine_path(envelope, EnginePath::Static)
                        }
                    }
                }
            }
        }
        (None, Some(html)) => match ctx.config.js_render {
            JsRenderMode::Off => apply_engine_path(
                extract_value(html, selector, fields, &options),
                EnginePath::Static,
            ),
            JsRenderMode::On => {
                finish_extract_fields(render_page(html), selector, fields, &options)
            }
            JsRenderMode::Auto => {
                let envelope = extract_value(html, selector, fields, &options);
                if static_extract_empty(&envelope) && auto_wants_render(html) {
                    finish_extract_fields(render_page(html), selector, fields, &options)
                } else {
                    apply_engine_path(envelope, EnginePath::Static)
                }
            }
        },
        (Some(_), Some(_)) | (None, None) => fail(
            ErrorCode::BadUrl,
            "url 与 html 只能给一个；extract 需要其中之一",
        ),
    };
    crate::resume::chunk_extract(envelope, params.start_index, params.char_budget)
}

async fn tool_search(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<SearchParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    let source = match resolve_source(ctx, params.url.as_deref(), params.html.as_deref()).await {
        Ok(source) => source,
        Err(value) => return value,
    };
    let mut options = SearchOptions::default();
    if let Some(value) = params.max_results {
        options.max_results = value;
    }
    if let Some(value) = params.context_chars {
        options.context_chars = value;
    }
    if let Some(value) = params.regex {
        options.regex = value;
    }
    if let Some(value) = params.case_insensitive {
        options.case_insensitive = value;
    }
    finish_source(
        to_value(extract::search(&source.html, &params.query, &options)),
        &source,
    )
}

async fn tool_links(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<LinksParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    let source = match resolve_source(ctx, params.url.as_deref(), params.html.as_deref()).await {
        Ok(source) => source,
        Err(value) => return value,
    };
    let filter = LinkFilter {
        domains: params.domains.unwrap_or_default(),
        rel: params.rel.unwrap_or_default(),
        pattern: params.pattern,
        internal_only: params.internal_only.unwrap_or(false),
        external_only: params.external_only.unwrap_or(false),
    };
    let mut options = ExtractOptions::default();
    if let Some(value) = params.max_results {
        options.max_results = value;
    }
    let base = params.base.as_deref().or(params.url.as_deref());
    finish_source(
        to_value(extract::links(&source.html, base, &filter, &options)),
        &source,
    )
}

async fn tool_tables(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<TablesParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    let source = match resolve_source(ctx, params.url.as_deref(), params.html.as_deref()).await {
        Ok(source) => source,
        Err(value) => return value,
    };
    let mut options = ExtractOptions::default();
    if let Some(value) = params.max_results {
        options.max_results = value;
    }
    finish_source(to_value(extract::tables(&source.html, &options)), &source)
}

async fn tool_forms(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<FormsParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    let source = match resolve_source(ctx, params.url.as_deref(), params.html.as_deref()).await {
        Ok(source) => source,
        Err(value) => return value,
    };
    let base = params.base.as_deref().or(params.url.as_deref());
    if let Some(index) = params.form_index {
        return finish_source(
            to_value(extract::form_by_index(&source.html, base, index)),
            &source,
        );
    }
    let mut options = ExtractOptions::default();
    if let Some(value) = params.max_results {
        options.max_results = value;
    }
    finish_source(
        to_value(extract::forms(&source.html, base, &options)),
        &source,
    )
}

async fn tool_fill_form(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<FillFormParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    if let Some(rejected) = offline_guard(ctx) {
        return rejected;
    }
    let session = match ctx.binding.resolve(params.session.as_deref()) {
        Ok(session) => session,
        Err(error) => return fail(ErrorCode::Blocked, error.to_string()),
    };
    let engine = match SessionEngine::with_store(&session, ctx.session_store()).map(|engine| {
        engine
            .with_browser_context(ctx.browser_context.clone())
            // #191：会话的 GET/POST 与 transport 使用同一地址守卫策略。
            .with_private_network_allowed(ctx.config.allow_private_network)
    }) {
        Ok(engine) => engine,
        Err(error) => return fail(ErrorCode::SessionLost, error.message()),
    };
    let envelope = engine.fetch_page(&params.url).await;
    if !envelope.ok {
        return to_value(envelope);
    }
    let page = envelope.data.expect("ok envelope has data");
    let mut fill = FormFill::new();
    for (name, value) in &params.fields {
        fill.set(name.clone(), value.clone());
    }
    if let Some(checks) = &params.checks {
        for (name, checked) in checks {
            fill.check(name.clone(), *checked);
        }
    }
    to_value(
        engine
            .submit_form(&page, params.form_index.unwrap_or(0), &fill)
            .await,
    )
}

async fn tool_paginate(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<PaginateParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    if let Some(rejected) = offline_guard(ctx) {
        return rejected;
    }
    let session = match ctx.binding.resolve(params.session.as_deref()) {
        Ok(session) => session,
        Err(error) => return fail(ErrorCode::Blocked, error.to_string()),
    };
    let engine = match SessionEngine::with_store(&session, ctx.session_store()).map(|engine| {
        engine
            .with_browser_context(ctx.browser_context.clone())
            // #191：会话的 GET/POST 与 transport 使用同一地址守卫策略。
            .with_private_network_allowed(ctx.config.allow_private_network)
    }) {
        Ok(engine) => engine,
        Err(error) => return fail(ErrorCode::SessionLost, error.message()),
    };
    to_value(engine.paginate(&params.url, params.max_pages).await)
}

fn require_name(params: &SessionParams) -> Result<String, Value> {
    params
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| fail(ErrorCode::Parse, "该动作必须给出会话名 name"))
}

fn mask_state(mut state: SessionState) -> SessionState {
    for cookie in &mut state.cookies {
        cookie.value = "<redacted>".to_owned();
    }
    state
}

async fn tool_session(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<SessionParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    if let Err(error) = ctx.binding.resolve(params.session.as_deref()) {
        return fail(ErrorCode::Blocked, error.to_string());
    }
    let store = ctx.session_store();
    match params.action {
        SessionAction::List => {
            let dir = store.sessions_dir();
            let mut names: Vec<String> = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) == Some("json")
                        && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
                    {
                        names.push(stem.to_owned());
                    }
                }
            }
            names.sort();
            to_value(Envelope::ok(
                json!({"sessions": names}),
                Trust::UntrustedPageContent,
                0,
            ))
        }
        SessionAction::Show | SessionAction::Load => {
            let name = match require_name(&params) {
                Ok(name) => name,
                Err(value) => return value,
            };
            match store.load(&name) {
                Ok(state) => to_value(Envelope::ok(
                    mask_state(state),
                    Trust::UntrustedPageContent,
                    0,
                )),
                Err(error) => fail(error.code(), error.message()),
            }
        }
        SessionAction::Save => {
            let name = match require_name(&params) {
                Ok(name) => name,
                Err(value) => return value,
            };
            let engine = match SessionEngine::with_store(&name, ctx.session_store())
                .map(|engine| engine.with_browser_context(ctx.browser_context.clone()))
            {
                Ok(engine) => engine,
                Err(error) => return fail(ErrorCode::SessionLost, error.message()),
            };
            match engine.save_state(&[]) {
                Ok(()) => to_value(Envelope::ok(
                    json!({
                        "saved": name,
                        "path": store.path_for(&name).map(|path| path.to_string_lossy().into_owned()).unwrap_or_default()
                    }),
                    Trust::UntrustedPageContent,
                    0,
                )),
                Err(error) => fail(error.code(), error.message()),
            }
        }
        SessionAction::Clear => {
            let name = match require_name(&params) {
                Ok(name) => name,
                Err(value) => return value,
            };
            match store.remove(&name) {
                Ok(removed) => to_value(Envelope::ok(
                    json!({"cleared": name, "removed": removed}),
                    Trust::UntrustedPageContent,
                    0,
                )),
                Err(error) => fail(error.code(), error.message()),
            }
        }
    }
}

async fn tool_cookies(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<CookiesParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    let session = match ctx.binding.resolve(params.session.as_deref()) {
        Ok(session) => session,
        Err(error) => return fail(ErrorCode::Blocked, error.to_string()),
    };
    let store = ctx.session_store();
    let state = match store.load(&session) {
        Ok(state) => state,
        Err(error) => return fail(error.code(), error.message()),
    };
    let host = params
        .url
        .as_deref()
        .and_then(|raw| Url::parse(raw).ok())
        .and_then(|url| url.host_str().map(str::to_owned));
    let cookies: Vec<Value> = state
        .cookies
        .iter()
        .filter(|cookie| match &host {
            Some(host) => host.ends_with(cookie.domain.trim_start_matches(".")),
            None => true,
        })
        .map(|cookie| {
            json!({
                "name": cookie.name,
                "value": "<redacted>",
                "domain": cookie.domain,
                "path": cookie.path,
                "secure": cookie.secure,
                "http_only": cookie.http_only,
                "expires_at": cookie.expires_at,
            })
        })
        .collect();
    match params.action {
        CookiesAction::View => to_value(Envelope::ok(
            json!({"session": session, "count": cookies.len(), "cookies": cookies}),
            Trust::UntrustedPageContent,
            0,
        )),
        CookiesAction::Export => {
            if params.yes != Some(true) {
                return fail(
                    ErrorCode::Blocked,
                    "cookie 导出被拒绝：必须显式传 yes=true（导出文件权限 0600）",
                );
            }
            match write_cookie_export(ctx.state_dir(), &session, &state) {
                Ok(path) => to_value(Envelope::ok(
                    json!({
                        "session": session,
                        "saved_to": path.to_string_lossy(),
                        "count": cookies.len(),
                        "values": "<redacted-in-response>"
                    }),
                    Trust::UntrustedPageContent,
                    0,
                )),
                Err(message) => fail(ErrorCode::Internal, message),
            }
        }
    }
}

fn write_cookie_export(
    state_dir: &Path,
    session: &str,
    state: &SessionState,
) -> Result<PathBuf, String> {
    let dir = state_dir.join("exports");
    std::fs::create_dir_all(&dir).map_err(|error| format!("创建导出目录失败：{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = dir.join(format!("cookies-{session}.json"));
    let text = state.export_json().map_err(|error| error.message())?;
    std::fs::write(&path, text).map_err(|error| format!("写入导出文件失败：{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

async fn read_source_bytes(
    ctx: &McpContext,
    json_text: Option<&str>,
    url: Option<&str>,
    path: Option<&str>,
) -> Result<(Vec<u8>, Option<UpstreamDegrade>), Value> {
    let given = [json_text.is_some(), url.is_some(), path.is_some()]
        .iter()
        .filter(|present| **present)
        .count();
    if given != 1 {
        return Err(fail(
            ErrorCode::BadUrl,
            "必须且只能给出 json / url / path 之一",
        ));
    }
    if let Some(text) = json_text {
        return Ok((text.as_bytes().to_vec(), None));
    }
    if let Some(path) = path {
        return std::fs::read(path)
            .map(|bytes| (bytes, None))
            .map_err(|error| fail(ErrorCode::BadUrl, format!("读取文件失败：{error}")));
    }
    let url = url.expect("checked above");
    if let Some(rejected) = offline_guard(ctx) {
        return Err(rejected);
    }
    let envelope = fetch_target(ctx, url).await;
    if !envelope.ok {
        return Err(to_value(envelope));
    }
    let upstream = UpstreamDegrade::capture(&envelope);
    let fetched = envelope.data.expect("ok envelope has data");
    match fetched.content.text {
        Some(text) => Ok((text.into_bytes(), upstream)),
        None => Err(to_value(Envelope::degraded(
            json!({"content_kind": format!("{:?}", fetched.content.kind).to_lowercase()}),
            "transport:raw-bytes-unavailable",
            ErrorCode::Unsupported,
            0,
        ))),
    }
}

async fn tool_json_query(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<JsonQueryParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    let (bytes, upstream) = match read_source_bytes(
        ctx,
        params.json.as_deref(),
        params.url.as_deref(),
        params.path.as_deref(),
    )
    .await
    {
        Ok(source) => source,
        Err(value) => return value,
    };
    apply_upstream_degradation(
        to_value(json_query(&bytes, &params.expression, &Default::default())),
        upstream.as_ref(),
    )
}

async fn tool_doc_text(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<DocTextParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    let (path, url) = (params.path.as_deref(), params.url.as_deref());
    let (bytes, upstream) = match read_source_bytes(ctx, None, url, path).await {
        Ok(source) => source,
        Err(value) => return value,
    };
    let value = apply_upstream_degradation(
        to_value(doc_text(&bytes, &DocOptions::default())),
        upstream.as_ref(),
    );
    crate::resume::chunk_doc_text(value, params.start_index, params.char_budget)
}

async fn tool_download(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<DownloadParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    if let Some(rejected) = offline_guard(ctx) {
        return rejected;
    }
    let mut options = ctx.fetch_options();
    if let Some(max_bytes) = params.max_bytes {
        options.max_body_bytes = max_bytes.max(1);
    }
    let envelope = fetch_target(ctx, &params.url).await;
    if !envelope.ok {
        return to_value(envelope);
    }
    let upstream = UpstreamDegrade::capture(&envelope);
    let fetched = envelope.data.expect("ok envelope has data");
    let Some(text) = fetched.content.text.as_deref() else {
        return to_value(Envelope::degraded(
            json!({
                "content_type": fetched.content.content_type,
                "byte_len": fetched.content.byte_len,
                "saved_to": Value::Null
            }),
            "transport:raw-bytes-unavailable",
            ErrorCode::Unsupported,
            0,
        ));
    };
    let dir = params
        .dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| ctx.state_dir().join("downloads"));
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return fail(ErrorCode::Internal, format!("创建下载目录失败：{error}"));
    }
    let filename = params
        .filename
        .clone()
        .or_else(|| default_filename(&params.url))
        .unwrap_or_else(|| "download.txt".to_owned());
    let path = dir.join(&filename);
    if let Err(error) = std::fs::write(&path, text.as_bytes()) {
        return fail(ErrorCode::Internal, format!("写入文件失败：{error}"));
    }
    apply_upstream_degradation(
        to_value(Envelope::ok(
            json!({
                "saved_to": path.to_string_lossy(),
                "bytes": text.len(),
                "status": fetched.status,
                "content_type": fetched.content.content_type,
            }),
            Trust::UntrustedPageContent,
            0,
        )),
        upstream.as_ref(),
    )
}

fn default_filename(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let last = parsed.path_segments()?.rfind(|part| !part.is_empty())?;
    let cleaned: String = last
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || ".-_".contains(*ch))
        .take(128)
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

async fn tool_robots(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<RobotsParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    if let Some(rejected) = offline_guard(ctx) {
        return rejected;
    }
    let url = match Url::parse(&params.url) {
        Ok(url) => url,
        Err(error) => return fail(ErrorCode::BadUrl, format!("非法 URL：{error}")),
    };
    if !matches!(url.scheme(), "http" | "https") {
        return fail(ErrorCode::BadUrl, format!("不支持的协议：{}", url.scheme()));
    }
    let config = PolitenessConfig {
        user_agent: params
            .user_agent
            .clone()
            .unwrap_or_else(|| PolitenessConfig::default().user_agent),
        ..PolitenessConfig::default()
    };
    let fetcher = PoliteFetcher::new(config, ctx.fetch_options());
    match fetcher.robots_rules(&url).await {
        Some(rules) => to_value(Envelope::ok(
            json!({"origin": url.origin().ascii_serialization(), "available": true, "rules": rules}),
            Trust::UntrustedPageContent,
            0,
        )),
        None => to_value(Envelope::ok(
            json!({
                "origin": url.origin().ascii_serialization(),
                "available": false,
                "note": "robots.txt 不存在或非 2xx：按无规则处理（服务端未声明限制）"
            }),
            Trust::UntrustedPageContent,
            0,
        )),
    }
}

async fn tool_caps(ctx: &McpContext, args: &Value) -> Value {
    let params = match parse::<CapsParams>(args) {
        Ok(params) => params,
        Err(value) => return value,
    };
    let tools = if params.include_tools.unwrap_or(true) {
        crate::caps::tool_capabilities()
    } else {
        Vec::new()
    };
    // #225：宿主声明的外部回退工具（未设置则 None，字段整体省略）。
    let report = crate::caps::report(&ctx.config, tools, webctl_core::fallback_decl::from_env());
    to_value(Envelope::ok(report, Trust::TrustedLocal, 0))
}
#[cfg(test)]
mod tests {
    use super::*;
    use webctl_web::content_type::ContentKind;
    use webctl_web::transport::{Body, Fetched, RedirectHop};

    const SECRET: &str = "SUPER_SECRET_TOKEN";

    /// 手工构造**仍含明文凭据**的 `Fetched`。
    ///
    /// 刻意**不经过** `webctl_web::transport::finish`（那里也会脱敏）——
    /// 否则本用例就会退化成"系统整体会脱敏"，证明不了 MCP 侧这次接入是否还有效。
    fn plaintext_fetched() -> Fetched {
        Fetched {
            status: 200,
            version: "http/1.1".to_owned(),
            final_url: format!("https://example.test/z?access_token={SECRET}&user=bob"),
            headers: vec![
                ("set-cookie".to_owned(), format!("sid={SECRET}; Path=/")),
                ("authorization".to_owned(), format!("Bearer {SECRET}")),
                ("content-type".to_owned(), "text/html".to_owned()),
            ],
            redirects: vec![RedirectHop {
                url: format!("https://example.test/a?token={SECRET}"),
                status: 302,
                location: format!("https://example.test/z?access_token={SECRET}"),
            }],
            content: Body {
                kind: ContentKind::Text,
                content_type: Some("text/html".to_owned()),
                byte_len: 0,
                declared_len: Some(0),
                truncated: false,
                text: Some("ok".to_owned()),
                charset: Some("UTF-8".to_owned()),
                charset_source: None,
                decode_had_errors: false,
            },
            elapsed_ms: 1,
        }
    }

    /// SEC-09 **隔离用例**：直接打 MCP 侧的接入点，不经过取回层。
    ///
    /// 反向对照（复核者必做）：把 `redact_fetched_envelope` 里的
    /// `webctl_web::redact::redact_fetched(data)` 删掉，本用例**必须失败**。
    /// 若它仍然通过，说明用例没有隔离住 MCP 侧接入。
    #[test]
    fn sec_09_mcp_dispatcher_is_the_one_that_redacts() {
        // 1) 先证明夹具里**确实**有明文（否则整条用例是空转）。
        let raw = plaintext_fetched();
        let raw_json = serde_json::to_string(&raw).expect("serialize raw");
        assert!(
            raw_json.contains(SECRET),
            "夹具构造有误：未脱敏数据里本应出现明文 SECRET"
        );

        // 2) 只调用 MCP 侧的接入点。
        let mut envelope = Envelope::ok(raw, Trust::UntrustedPageContent, 0);
        redact_fetched_envelope(&mut envelope);
        let data = envelope.data.expect("data");
        let json = serde_json::to_string(&data).expect("serialize redacted");

        // 3) 明文必须消失，而**非敏感头必须原样保留**（证明是定向脱敏，不是整段抹掉）。
        assert!(
            !json.contains(SECRET),
            "MCP 侧脱敏未生效，仍可见明文：{json}"
        );
        assert!(
            data.headers
                .iter()
                .any(|(name, value)| name == "content-type" && value == "text/html"),
            "非敏感头必须保持原样"
        );
        assert_eq!(envelope.meta.trust, Trust::UntrustedPageContent);
    }

    /// #192：auto 的触发判定只认 degrade 的“需要 JS”结论，静态页不得误伤。
    #[test]
    fn auto_render_trigger_only_matches_js_shell() {
        let shell =
            include_str!("../../webctl-web/tests/fixtures/degrade/js_data_script_shell.html");
        let static_page =
            include_str!("../../webctl-web/tests/fixtures/degrade/static_quotes_10.html");
        assert!(auto_wants_render(shell), "脚本主导的空壳页应触发渲染");
        assert!(
            !auto_wants_render(static_page),
            "静态正文充分的页面绝不能触发渲染"
        );
    }
}
