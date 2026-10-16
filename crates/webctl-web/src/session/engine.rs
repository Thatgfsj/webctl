//! 协议级会话引擎（PLAN v0.4 §2「会话」域，PR 04）。
//!
//! 边界（必须诚实）：只做服务端渲染页面的取回、表单提交与分页；没有 JS 引擎。
//! 因此：
//! - GET 复用 PR 02 的 transport::fetch（重定向链、cookie jar、编码、内容类型路由）；
//! - 提交用 wreq 直接发 POST（transport 只暴露 GET），但仍手工跟随重定向、
//!   复用同一个 CookieJar、把错误归一为封闭错误码；
//! - SPA/JS 页面显式 DEGRADED（code + meta.degraded_from），绝不伪造成功；
//! - 未登录（落在登录表单上）返回 SESSION_LOST，与 BLOCKED（HTTP 拒绝）区分。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use url::Url;
use webctl_core::{Envelope, ErrorCode, Trust};
use wreq::redirect::Policy;

use crate::content_type::{self, ContentKind};
use crate::encoding;
use crate::transport::{CookieJar, FetchOptions, Fetched, fetch};

use super::csrf;
use super::form::{self, Form, FormFill};
use super::html;
use super::pagination::{self, NextLink};
use super::spa::{self, SpaSignal};
use super::state::{SessionError, SessionState, SessionStore};

/// 默认分页上限。
pub const DEFAULT_MAX_PAGES: usize = 20;
/// 默认用于统计条目数的元素名。
pub const DEFAULT_ITEM_ELEMENT: &str = "li";

/// 一个已解析的页面。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    /// 最终 URL（跟随重定向之后）。
    pub url: String,
    /// 最终状态码。
    pub status: u16,
    /// 内容类型分支。
    pub kind: ContentKind,
    /// 原始 Content-Type 头。
    pub content_type: Option<String>,
    /// 标题（若有）。
    pub title: Option<String>,
    /// HTML 原文（非 HTML 分支为空串）。
    pub html: String,
    /// 页面里的表单。
    pub forms: Vec<Form>,
    /// 识别到的下一页链接。
    pub next: Option<NextLink>,
    /// SPA 判定证据（None 表示未判定为 SPA）。
    pub spa: Option<SpaSignal>,
    /// 条目数（按 item_element 统计，用于分页求和）。
    pub item_count: usize,
    /// 可见文本字符数（诊断用）。
    pub visible_text_len: usize,
}

impl Page {
    /// 是否落在登录表单上（未登录的强信号）。
    pub fn looks_like_login(&self) -> bool {
        self.forms.iter().any(Form::looks_like_login)
    }

    /// 取第 index 个表单。
    pub fn form(&self, index: usize) -> Option<&Form> {
        self.forms.get(index)
    }
}

/// 分页遍历结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paginated {
    /// 依次取到的页面。
    pub pages: Vec<Page>,
    /// 每页条目数（与 pages 等长）。
    pub item_counts: Vec<usize>,
    /// 条目总数（item_counts 之和）。
    pub total_items: usize,
    /// 命中上限时的人类可读原因（未命中为 None）。
    pub limit: Option<String>,
    /// 停止时仍未取回的下一页（若有）。
    pub next: Option<String>,
}

/// HTTP 响应原始部件。
struct ResponseParts {
    url: String,
    status: u16,
    content_type: Option<String>,
    body: Vec<u8>,
    truncated: bool,
}

/// 协议级会话引擎。
pub struct SessionEngine {
    name: String,
    jar: Arc<CookieJar>,
    options: FetchOptions,
    store: SessionStore,
    max_pages: usize,
    item_element: String,
    last_token: Mutex<Option<(String, String)>>,
    logged_in: AtomicBool,
}

impl std::fmt::Debug for SessionEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionEngine")
            .field("name", &self.name)
            .field("cookies", &self.jar.len())
            .field("max_pages", &self.max_pages)
            .finish_non_exhaustive()
    }
}

impl SessionEngine {
    /// 用默认 state dir 新建会话引擎。
    pub fn new(name: &str) -> Result<Self, SessionError> {
        let store = SessionStore::new(webctl_core::config::default_state_dir());
        Self::with_store(name, store)
    }

    /// 指定存储新建会话引擎。
    pub fn with_store(name: &str, store: SessionStore) -> Result<Self, SessionError> {
        store.path_for(name)?;
        let jar = Arc::new(CookieJar::new());
        let options = FetchOptions::default().with_cookie_jar(Arc::clone(&jar));
        Ok(Self {
            name: name.to_owned(),
            jar,
            options,
            store,
            max_pages: DEFAULT_MAX_PAGES,
            item_element: DEFAULT_ITEM_ELEMENT.to_owned(),
            last_token: Mutex::new(None),
            logged_in: AtomicBool::new(false),
        })
    }

    /// 若磁盘上已有未过期会话则恢复，否则新建空会话。
    pub fn restore(name: &str, store: SessionStore) -> Result<Self, SessionError> {
        let state = match store.load(name) {
            Ok(state) => Some(state),
            Err(SessionError::NotFound(_)) => None,
            Err(other) => return Err(other),
        };
        let mut engine = Self::with_store(name, store)?;
        if let Some(state) = state {
            engine.import_state(&state);
        }
        Ok(engine)
    }

    /// 会话名。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 共享 cookie jar（供并发只读使用）。
    pub fn cookie_jar(&self) -> &Arc<CookieJar> {
        &self.jar
    }

    /// 存储句柄。
    pub fn store(&self) -> &SessionStore {
        &self.store
    }

    /// 分页上限。
    pub fn max_pages(&self) -> usize {
        self.max_pages
    }

    /// 设置分页上限。
    pub fn set_max_pages(&mut self, max_pages: usize) {
        self.max_pages = max_pages.max(1);
    }

    /// 条目统计元素名。
    pub fn item_element(&self) -> &str {
        &self.item_element
    }

    /// 设置条目统计元素名。
    pub fn set_item_element(&mut self, element: impl Into<String>) {
        self.item_element = element.into();
    }

    /// 是否已成功登录过（会话内标志）。
    pub fn is_logged_in(&self) -> bool {
        self.logged_in.load(Ordering::SeqCst)
    }

    /// 导出当前会话状态（结构层面）。
    pub fn export_state(&self, origins: &[String]) -> SessionState {
        let mut state = SessionState::new(&self.name, self.store.ttl());
        state.cookies = self.jar.snapshot();
        state.origins = origins.to_vec();
        state
    }

    /// 用导入的状态替换 cookie（不触碰磁盘）。
    pub fn import_state(&mut self, state: &SessionState) {
        let jar = Arc::new(CookieJar::from_snapshot(state.cookies.clone()));
        self.options = self.options.clone().with_cookie_jar(Arc::clone(&jar));
        self.jar = jar;
    }

    /// 落盘当前会话（cookie 持久化）。
    pub fn save_state(&self, origins: &[String]) -> Result<(), SessionError> {
        let state = self.export_state(origins);
        self.store.save(&state)?;
        Ok(())
    }

    /// 从磁盘加载会话到当前引擎。
    pub fn load_state(&mut self) -> Result<(), SessionError> {
        let state = self.store.load(&self.name)?;
        self.import_state(&state);
        Ok(())
    }

    /// GET 一页（复用 transport::fetch）。
    pub async fn fetch_page(&self, url: &str) -> Envelope<Page> {
        let env = fetch(url, &self.options).await;
        let elapsed = env.meta.elapsed_ms;
        let degraded_from = env.meta.degraded_from.clone();
        let code = env.code;
        match env.data {
            None => Envelope::fail(code, env.error, elapsed),
            Some(fetched) => {
                let page = page_from_fetched(fetched, &self.item_element);
                self.wrap_page(page, code, degraded_from, elapsed)
            }
        }
    }

    /// GET 一页，并要求它不是登录页（未登录返回 SESSION_LOST）。
    pub async fn fetch_authenticated(&self, url: &str) -> Envelope<Page> {
        let env = self.fetch_page(url).await;
        if let Some(page) = &env.data
            && page.looks_like_login()
        {
            return Envelope::fail(
                ErrorCode::SessionLost,
                format!(
                    "not authenticated: {url} landed on a login form (session {:?})",
                    self.name
                ),
                env.meta.elapsed_ms,
            );
        }
        env
    }

    /// 提交表单。
    pub async fn submit_form(
        &self,
        page: &Page,
        form_index: usize,
        user_fill: &FormFill,
    ) -> Envelope<Page> {
        let started = Instant::now();
        let Some(form) = page.form(form_index) else {
            return Envelope::fail(
                ErrorCode::Parse,
                format!(
                    "page has {} form(s); form_index {form_index} is out of range",
                    page.forms.len()
                ),
                elapsed_ms(started),
            );
        };
        let mut fill = form.default_fill();
        fill.merge(user_fill);
        csrf::refill_from_html(&mut fill, form, &page.html);
        let encoded = match form.encode(&fill) {
            Ok(encoded) => encoded,
            Err(error) => {
                return Envelope::fail(error.code(), error.message(), elapsed_ms(started));
            }
        };
        let action = match resolve_action(&page.url, &form.action) {
            Ok(action) => action,
            Err(message) => return Envelope::fail(ErrorCode::BadUrl, message, elapsed_ms(started)),
        };
        let token = csrf::extract_form_tokens(form)
            .into_iter()
            .map(|token| token.value)
            .next();
        if let Some(token) = &token
            && self.is_duplicate_token(&action, token)
        {
            return Envelope::fail(
                ErrorCode::Blocked,
                format!(
                    "duplicate submission refused: CSRF token for {action} was already used \
                     without re-fetching the page"
                ),
                elapsed_ms(started),
            );
        }

        let result = match form.method {
            form::FormMethod::Get => {
                let url = append_query(&action, &encoded.bytes);
                self.send(wreq::Method::GET, &url, None).await
            }
            form::FormMethod::Post => {
                self.send(
                    wreq::Method::POST,
                    &action,
                    Some((encoded.content_type.as_str(), encoded.bytes.clone())),
                )
                .await
            }
        };
        let parts = match result {
            Ok(parts) => parts,
            Err(error) => return Envelope::fail(error.code, error.message, elapsed_ms(started)),
        };
        if let Some(token) = token {
            self.remember_token(&action, token);
        }
        let truncated = parts.truncated;
        let page = page_from_parts(parts, &self.item_element);
        if truncated {
            return Envelope::degraded(
                page,
                "response-body-truncated",
                ErrorCode::Limit,
                elapsed_ms(started),
            );
        }
        self.wrap_page(page, ErrorCode::Ok, None, elapsed_ms(started))
    }

    /// 登录：取登录页、填凭据、提交，并确认已离开登录表单。
    pub async fn login(
        &self,
        url: &str,
        form_index: Option<usize>,
        credentials: &FormFill,
    ) -> Envelope<Page> {
        let env = self.fetch_page(url).await;
        let Some(page) = env.data.clone() else {
            return env;
        };
        let index = match form_index {
            Some(index) => index,
            None => page
                .forms
                .iter()
                .position(Form::looks_like_login)
                .unwrap_or(0),
        };
        if page.forms.is_empty() {
            return Envelope::fail(
                ErrorCode::Parse,
                format!("no form found at {url}; cannot log in"),
                env.meta.elapsed_ms,
            );
        }
        let submitted = self.submit_form(&page, index, credentials).await;
        if !submitted.ok {
            return submitted;
        }
        let landed = submitted.data.as_ref().expect("ok envelope has data");
        if landed.looks_like_login() {
            return Envelope::fail(
                ErrorCode::SessionLost,
                format!(
                    "login rejected: still on a login form after submitting (session {:?})",
                    self.name
                ),
                submitted.meta.elapsed_ms,
            );
        }
        self.logged_in.store(true, Ordering::SeqCst);
        submitted
    }

    /// 从 start_url 起遍历 next 链接，最多 max_pages 页（None 用引擎默认）。
    ///
    /// 降级传播（禁止静默降级）：只要任意一页的 fetch_page 返回了降级
    /// （SPA/js-render、响应体截断等），整个分页结果也必须 DEGRADED 并带上
    /// 非空的 `meta.degraded_from`——即使只取到一页。命页数上限/循环时同样
    /// 以 LIMIT 降级返回。绝不把空壳页当成 OK+0 条。
    pub async fn paginate(&self, start_url: &str, max_pages: Option<usize>) -> Envelope<Paginated> {
        let started = Instant::now();
        let max = max_pages.unwrap_or(self.max_pages).max(1);
        let mut visited: Vec<String> = Vec::new();
        let mut pages: Vec<Page> = Vec::new();
        let mut limit: Option<&str> = None;
        let mut cursor = Some(start_url.to_owned());
        // 第一处被观测到的页面级降级：(degraded_from, code)。
        let mut degraded: Option<(String, ErrorCode)> = None;

        while let Some(url) = cursor.clone() {
            if visited.iter().any(|seen| seen == &url) {
                limit = Some("pagination-loop");
                break;
            }
            if pages.len() >= max {
                limit = Some("pagination-max-pages");
                break;
            }
            visited.push(url.clone());
            let env = self.fetch_page(&url).await;
            let code = env.code;
            let degraded_from = env.meta.degraded_from.clone();
            let Some(page) = env.data else {
                if pages.is_empty() {
                    return Envelope::fail(code, env.error, elapsed_ms(started));
                }
                return Envelope::degraded(
                    assemble(pages, cursor, Some("pagination-fetch-failed")),
                    "pagination-fetch-failed",
                    ErrorCode::Degraded,
                    elapsed_ms(started),
                );
            };
            if degraded.is_none() {
                match (degraded_from, code.is_degradation()) {
                    (Some(from), _) => degraded = Some((from, code)),
                    // 兜底：不进降级码却缺 degraded_from 的信封不可能是正常页。
                    (None, true) => {
                        degraded = Some(("pagination-degraded-page".to_owned(), code));
                    }
                    (None, false) => {}
                }
            }
            cursor = page.next.as_ref().map(|next| next.url.clone());
            pages.push(page);
        }

        let paginated = assemble(pages, cursor, limit);
        match (limit, degraded) {
            (Some(reason), Some((from, code))) => Envelope::degraded(
                paginated,
                format!("{from}+{reason}"),
                code,
                elapsed_ms(started),
            ),
            (Some(reason), None) => {
                Envelope::degraded(paginated, reason, ErrorCode::Limit, elapsed_ms(started))
            }
            (None, Some((from, code))) => {
                Envelope::degraded(paginated, from, code, elapsed_ms(started))
            }
            (None, None) => {
                Envelope::ok(paginated, Trust::UntrustedPageContent, elapsed_ms(started))
            }
        }
    }

    fn wrap_page(
        &self,
        page: Page,
        code: ErrorCode,
        degraded_from: Option<String>,
        elapsed: u64,
    ) -> Envelope<Page> {
        if let Some(from) = degraded_from {
            return Envelope::degraded(page, from, code, elapsed);
        }
        if page.spa.is_some() {
            return Envelope::degraded(
                page,
                SpaSignal::DEGRADED_FROM,
                ErrorCode::Degraded,
                elapsed,
            );
        }
        Envelope::ok(page, Trust::UntrustedPageContent, elapsed)
    }

    fn is_duplicate_token(&self, action: &str, token: &str) -> bool {
        let last = self.lock_last_token();
        last.as_ref()
            .is_some_and(|(last_action, last_token)| last_action == action && last_token == token)
    }

    fn remember_token(&self, action: &str, token: String) {
        let mut last = self.lock_last_token();
        *last = Some((action.to_owned(), token));
    }

    fn lock_last_token(&self) -> std::sync::MutexGuard<'_, Option<(String, String)>> {
        self.last_token
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 直接发送请求（手工跟随重定向、复用 cookie jar）。
    async fn send(
        &self,
        method: wreq::Method,
        target: &str,
        body: Option<(&str, Vec<u8>)>,
    ) -> Result<ResponseParts, RequestError> {
        let url = Url::parse(target).map_err(|error| {
            RequestError::new(ErrorCode::BadUrl, format!("invalid url: {error}"))
        })?;
        let client = wreq::Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .timeout(self.options.timeout)
            .build()
            .map_err(|error| {
                RequestError::new(ErrorCode::Internal, format!("client init failed: {error}"))
            })?;

        let mut current = url;
        let mut hops = 0usize;
        let mut visited: Vec<String> = vec![current.to_string()];
        let mut current_method = method;
        let mut current_body = body;

        loop {
            let mut request = client.request(current_method.clone(), current.as_str());
            if let Some(user_agent) = &self.options.user_agent {
                request = request.header(wreq::header::USER_AGENT, user_agent.as_str());
            }
            if let Some(header) = self.jar.cookie_header(&current) {
                request = request.header(wreq::header::COOKIE, header);
            }
            for (name, value) in &self.options.headers {
                request = request.header(name.as_str(), value.as_str());
            }
            // 重定向可能重发同一请求体（307/308），因此这里只能借用而不能 take()；
            // 303（以及 POST 的 301/302）会在下面把 current_body 清空，转成无体 GET。
            if let Some((content_type, bytes)) = &current_body {
                request = request
                    .header(wreq::header::CONTENT_TYPE, *content_type)
                    .body(bytes.clone());
            }
            let response = request.send().await.map_err(map_wreq_error)?;
            let set_cookies: Vec<String> = response
                .headers()
                .get_all(wreq::header::SET_COOKIE)
                .iter()
                .filter_map(|value| value.to_str().ok())
                .map(str::to_owned)
                .collect();
            if !set_cookies.is_empty() {
                self.jar.store_set_cookies(&set_cookies, &current);
            }

            let status = response.status();
            if status.is_redirection()
                && let Some(location) = response
                    .headers()
                    .get(wreq::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned)
            {
                let next = current.join(&location).map_err(|error| {
                    RequestError::new(
                        ErrorCode::BadUrl,
                        format!("invalid redirect Location {location:?}: {error}"),
                    )
                })?;
                hops += 1;
                if hops > self.options.max_redirects {
                    return Err(RequestError::new(
                        ErrorCode::Network,
                        format!(
                            "redirect limit exceeded (max {})",
                            self.options.max_redirects
                        ),
                    ));
                }
                if visited.iter().any(|seen| seen == next.as_str()) {
                    return Err(RequestError::new(
                        ErrorCode::Network,
                        format!("redirect loop detected after {hops} hop(s)"),
                    ));
                }
                visited.push(next.to_string());
                // 浏览器语义：303 与 POST 的 301/302 改为 GET；307/308 保留方法。
                let status_code = status.as_u16();
                let switch_to_get = status_code == 303
                    || (status_code != 307
                        && status_code != 308
                        && current_method != wreq::Method::GET);
                if switch_to_get {
                    current_method = wreq::Method::GET;
                    current_body = None;
                }
                current = next;
                continue;
            }

            if status.is_client_error() || status.is_server_error() {
                return Err(RequestError::new(
                    ErrorCode::Blocked,
                    format!(
                        "HTTP {} {} at {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("Unknown"),
                        current
                    ),
                ));
            }

            let headers = header_pairs(response.headers());
            let content_type = headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                .map(|(_, value)| value.clone());
            let (bytes, truncated) = read_body(response, self.options.max_body_bytes).await?;
            return Ok(ResponseParts {
                url: current.to_string(),
                status: status.as_u16(),
                content_type,
                body: bytes,
                truncated,
            });
        }
    }
}

fn assemble(pages: Vec<Page>, next: Option<String>, limit: Option<&str>) -> Paginated {
    let item_counts: Vec<usize> = pages.iter().map(|page| page.item_count).collect();
    let total_items = item_counts.iter().sum();
    Paginated {
        pages,
        item_counts,
        total_items,
        limit: limit.map(str::to_owned),
        next,
    }
}

fn resolve_action(base: &str, action: &str) -> Result<String, String> {
    let base = Url::parse(base).map_err(|error| format!("invalid base url {base:?}: {error}"))?;
    let action = action.trim();
    if action.is_empty() || action.starts_with('#') {
        return Ok(base.to_string());
    }
    base.join(action)
        .map(|url| url.to_string())
        .map_err(|error| format!("invalid form action {action:?}: {error}"))
}

fn append_query(url: &str, body: &[u8]) -> String {
    let query = String::from_utf8_lossy(body);
    if query.is_empty() {
        return url.to_owned();
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}{query}")
}

fn page_from_fetched(fetched: Fetched, item_element: &str) -> Page {
    let parts = ResponseParts {
        url: fetched.final_url,
        status: fetched.status,
        content_type: fetched.content.content_type.clone(),
        body: Vec::new(),
        truncated: fetched.content.truncated,
    };
    let text = fetched.content.text.clone().unwrap_or_default();
    build_page(parts, Some(text), item_element, fetched.content.kind)
}

fn page_from_parts(parts: ResponseParts, item_element: &str) -> Page {
    let kind = content_type::classify(parts.content_type.as_deref(), &parts.body);
    build_page(parts, None, item_element, kind)
}

fn build_page(
    parts: ResponseParts,
    predecoded: Option<String>,
    item_element: &str,
    kind: ContentKind,
) -> Page {
    let text = if kind.is_text() {
        predecoded.unwrap_or_else(|| {
            encoding::decode_body(&parts.body, parts.content_type.as_deref()).text
        })
    } else {
        String::new()
    };
    let html = if kind == ContentKind::Html {
        text.clone()
    } else {
        String::new()
    };
    let nodes = html::tokenize(&html);
    let title = extract_title(&nodes);
    let forms = if html.is_empty() {
        Vec::new()
    } else {
        form::parse_forms(&html)
    };
    let next = Url::parse(&parts.url)
        .ok()
        .and_then(|base| pagination::find_next(&html, &base));
    let spa = if html.is_empty() {
        None
    } else {
        spa::detect(&html)
    };
    let item_count = html::count_tags(&nodes, item_element);
    let visible_text_len = html::text_content(&nodes)
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .count();
    Page {
        url: parts.url,
        status: parts.status,
        kind,
        content_type: parts.content_type,
        title,
        html,
        forms,
        next,
        spa,
        item_count,
        visible_text_len,
    }
}

fn extract_title(nodes: &[html::Node]) -> Option<String> {
    let mut in_title = false;
    let mut title = String::new();
    for node in nodes {
        match node {
            html::Node::Tag(tag) if tag.name == "title" => {
                if tag.closing {
                    if in_title {
                        break;
                    }
                } else {
                    in_title = true;
                }
            }
            html::Node::Text(text) if in_title => title.push_str(text),
            _ => {}
        }
    }
    let title = title.trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_owned())
    }
}

async fn read_body(
    response: wreq::Response,
    max_body_bytes: usize,
) -> Result<(Vec<u8>, bool), RequestError> {
    let mut stream = response.bytes_stream();
    let mut body: Vec<u8> = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(map_wreq_error)?;
        let remaining = max_body_bytes.saturating_sub(body.len());
        if remaining == 0 {
            truncated = true;
            break;
        }
        if chunk.len() > remaining {
            body.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, truncated))
}

fn header_pairs(headers: &wreq::header::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value
                    .to_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|_| String::from_utf8_lossy(value.as_bytes()).into_owned()),
            )
        })
        .collect()
}

fn map_wreq_error(error: wreq::Error) -> RequestError {
    if error.is_timeout() {
        return RequestError::new(ErrorCode::Timeout, format!("request timed out: {error}"));
    }
    if error.is_decode() {
        return RequestError::new(
            ErrorCode::Parse,
            format!("response body decode failed: {error}"),
        );
    }
    if error.is_connect() {
        return RequestError::new(ErrorCode::Network, format!("connection failed: {error}"));
    }
    RequestError::new(ErrorCode::Network, format!("transport error: {error}"))
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestError {
    code: ErrorCode,
    message: String,
}

impl RequestError {
    fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// 供测试与调用方复用的超时常量。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
