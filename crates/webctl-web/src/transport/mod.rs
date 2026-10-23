//! HTTP 取回层（PLAN v0.4 §2「取回」域，PR 02）。
//!
//! 设计要点：
//! - 只把字节拿回来并分类，不做正文提取（PR 03）。
//! - 重定向**手动跟随**：`redirect(Policy::none())` + 自建循环，每一跳都记录在结果里；
//!   相对 `Location` 按当前 URL 解析；有 max-hops 上限；重定向环会被检出并终止。
//! - cookie jar 为自实现的最小实现（见 [`cookies`]），跨请求保持且可序列化往返。
//! - 内容编码（gzip / brotli / deflate）由 wreq 内容编码层解压；
//!   本层负责**字符编码**与**内容类型路由**。
//! - 所有返回走 [`webctl_core::Envelope`]；页面内容固定 `Trust::UntrustedPageContent`。
//! - 零子进程（PLAN v0.4 §3.1）：只用进程内 TCP/TLS，不创建 `std::process`。

pub mod cookies;

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use url::Url;
use webctl_core::{Envelope, ErrorCode, Trust};
use wreq::redirect::Policy;

use crate::browser_context::{BrowserContext, inject_headers};
use crate::content_type::{self, ContentKind};
use crate::encoding::{self, CharsetSource};

pub use cookies::{CookieJar, StoredCookie};

/// 默认单跳超时（毫秒预算之外的兜底）。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// 默认最大重定向跳数。
pub const DEFAULT_MAX_REDIRECTS: usize = 5;
/// 默认响应体上限（8 MiB）：超过即截断并标 `LIMIT`。
pub const DEFAULT_MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// 取回参数。
///
/// `proxy` 为显式代理 URL；`None` 时固定 `no_proxy`（直连），
/// 保证离线测试不被宿主环境代理劫持。代理值来自 PR 10 的配置层。
#[derive(Debug, Clone)]
pub struct FetchOptions {
    /// 单次请求超时（含连接与读取）。
    pub timeout: Duration,
    /// 允许跟随的最大重定向跳数。
    pub max_redirects: usize,
    /// 响应体上限；超过即截断。
    pub max_body_bytes: usize,
    /// 额外请求头（按顺序附加）。
    pub headers: Vec<(String, String)>,
    /// 覆盖默认 User-Agent。
    pub user_agent: Option<String>,
    /// 是否跟随重定向（默认真）。
    pub follow_redirects: bool,
    /// 跨请求共享的 cookie jar。
    pub cookie_jar: Option<Arc<CookieJar>>,
    /// 显式代理 URL；`None` 表示不使用代理（直连）。
    pub proxy: Option<String>,
    /// 宿主注入的浏览器上下文（0.2 M0 / ADR-0006）；`None` 表示不注入任何头。
    ///
    /// 只借用 HTTP 头与 Cookie；TLS/JS 指纹**不可用**。凭据类头跨源不携带（见 `inject_headers`）。
    pub browser_context: Option<BrowserContext>,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            max_redirects: DEFAULT_MAX_REDIRECTS,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            headers: Vec::new(),
            user_agent: None,
            follow_redirects: true,
            cookie_jar: None,
            proxy: None,
            browser_context: None,
        }
    }
}

impl FetchOptions {
    /// 挂上一个可与其它请求共享的 cookie jar。
    pub fn with_cookie_jar(mut self, jar: Arc<CookieJar>) -> Self {
        self.cookie_jar = Some(jar);
        self
    }
}

/// 一跳重定向：产生该跳的 URL、状态码与解析后的目标。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedirectHop {
    /// 发出该响应的 URL。
    pub url: String,
    /// 重定向状态码（301/302/303/307/308…）。
    pub status: u16,
    /// 解析后的绝对目标 URL（相对 `Location` 已展开）。
    pub location: String,
}

/// 响应体：文本分支带解码结果，二进制分支只带元数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    /// 内容类型分支。
    pub kind: ContentKind,
    /// 原始 `Content-Type` 头。
    pub content_type: Option<String>,
    /// 实际保留的字节数（截断后为上限值）。
    pub byte_len: usize,
    /// 服务端声明的长度（若有）。
    pub declared_len: Option<u64>,
    /// 是否因命中 `max_body_bytes` 被截断。
    pub truncated: bool,
    /// 文本分支的解码结果；二进制分支恒为 `None`。
    pub text: Option<String>,
    /// 文本分支实际使用的编码名。
    pub charset: Option<String>,
    /// 文本分支的编码判定来源。
    pub charset_source: Option<CharsetSource>,
    /// 文本解码是否出现过非法序列。
    pub decode_had_errors: bool,
}

/// 一次取回的完整结果（成功时放进信封的 `data`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fetched {
    /// 最终响应状态码。
    pub status: u16,
    /// 最终响应的 HTTP 版本（`http/1.1` / `http/2`…）。
    pub version: String,
    /// 最终 URL（重定向后）。
    pub final_url: String,
    /// 最终响应的头（保序、允许重名）。
    pub headers: Vec<(String, String)>,
    /// 重定向链：每跳的来源 URL 与状态码；未跟随则只含已发生的跳。
    pub redirects: Vec<RedirectHop>,
    /// 响应体。
    pub content: Body,
    /// 本层实测耗时（毫秒）。
    pub elapsed_ms: u64,
}

/// 取回一个 http(s) URL。
///
/// 非法 URL 在**发起任何连接之前**返回 `BAD_URL`。
pub async fn fetch(target: &str, options: &FetchOptions) -> Envelope<Fetched> {
    let started = Instant::now();
    let url = match parse_target(target) {
        Ok(url) => url,
        Err(message) => {
            return Envelope::fail(
                ErrorCode::BadUrl,
                crate::redact::redact_log_line(&message),
                elapsed_ms(started),
            );
        }
    };
    match run(url, options, true).await {
        Ok(fetched) => {
            let envelope = if fetched.content.truncated {
                Envelope::degraded(
                    fetched,
                    "response-body-truncated",
                    ErrorCode::Limit,
                    elapsed_ms(started),
                )
            } else {
                Envelope::ok(fetched, Trust::UntrustedPageContent, elapsed_ms(started))
            };
            degrade_if_js_dependent(envelope)
        }
        Err(error) => Envelope::fail(
            error.code,
            crate::redact::redact_log_line(&error.message),
            elapsed_ms(started),
        ),
    }
}

/// 对成功取回的信封做 JS 依赖判定（PR 09）：只有 HTML 文本才判定，其余原样返回。
///
/// 这是 PR 09 在取回层的**唯一**接入点：响应体原样保留，只在页面需要 JS 时把
/// code / error / meta.degraded_from 写成显式降级（禁止静默降级）。`fetch_raw`
/// 是礼貌层读错误响应头的逃生口，刻意不接入，保持原样返回。
fn degrade_if_js_dependent(envelope: Envelope<Fetched>) -> Envelope<Fetched> {
    let html = envelope
        .data
        .as_ref()
        .filter(|fetched| fetched.content.kind == ContentKind::Html)
        .and_then(|fetched| fetched.content.text.clone());
    match html {
        Some(html) => crate::degrade::guard(
            &html,
            envelope,
            &crate::degrade::DegradeOptions::default(),
            crate::degrade::Scope::Fetch,
        ),
        None => envelope,
    }
}

/// 与 [`fetch`] 相同，但**不把 4xx/5xx 当作失败**：状态码、响应头与响应体原样返回。
///
/// 存在的理由：上层（礼貌层）需要读 `Retry-After` 之类的**错误响应头**来做退避与拦截判定，
/// 而失败信封按不变量不允许携带 `data`。非法 URL 仍返回 `BAD_URL`，网络/超时/重定向错误
/// 仍返回对应错误码——只有"HTTP 状态码本身"不再被折叠成 `BLOCKED`。
pub async fn fetch_raw(target: &str, options: &FetchOptions) -> Envelope<Fetched> {
    let started = Instant::now();
    let url = match parse_target(target) {
        Ok(url) => url,
        Err(message) => {
            return Envelope::fail(
                ErrorCode::BadUrl,
                crate::redact::redact_log_line(&message),
                elapsed_ms(started),
            );
        }
    };
    match run(url, options, false).await {
        Ok(fetched) => {
            if fetched.content.truncated {
                Envelope::degraded(
                    fetched,
                    "response-body-truncated",
                    ErrorCode::Limit,
                    elapsed_ms(started),
                )
            } else {
                Envelope::ok(fetched, Trust::UntrustedPageContent, elapsed_ms(started))
            }
        }
        Err(error) => Envelope::fail(
            error.code,
            crate::redact::redact_log_line(&error.message),
            elapsed_ms(started),
        ),
    }
}

fn parse_target(target: &str) -> Result<Url, String> {
    let url = Url::parse(target).map_err(|error| format!("invalid url {target:?}: {error}"))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(format!(
            "unsupported scheme {other:?}: only http/https are allowed"
        )),
    }
}

async fn run(
    start: Url,
    options: &FetchOptions,
    http_errors_are_failure: bool,
) -> Result<Fetched, TransportError> {
    let started = Instant::now();
    let client = build_client(options)?;
    let initial = start.clone();
    let mut current = start;
    let mut hops: Vec<RedirectHop> = Vec::new();
    let mut visited: Vec<String> = vec![current.as_str().to_owned()];

    loop {
        let mut request = client.request(wreq::Method::GET, current.as_str());
        if let Some(user_agent) = &options.user_agent {
            request = request.header(wreq::header::USER_AGENT, user_agent.as_str());
        }
        if let Some(jar) = &options.cookie_jar
            && let Some(header) = jar.cookie_header(&current)
        {
            request = request.header(wreq::header::COOKIE, header);
        }
        // SEC-06：跨源重定向不得携带原始凭据（Authorization / Cookie / API key）。
        // cookie jar 自身按域作用域下发；这里额外挡住调用方显式传入的敏感头。
        let cross_origin = !same_origin(&initial, &current);
        for (name, value) in &options.headers {
            if cross_origin && webctl_core::security::redact::is_sensitive_header(name) {
                continue;
            }
            request = request.header(name.as_str(), value.as_str());
        }

        if let Some(context) = &options.browser_context {
            let host = current.host_str().unwrap_or_default();
            // SEC-06：跨源重定向不携带凭据类头（cookie/referer），由 inject_headers 负责过滤。
            for (name, value) in inject_headers(
                context,
                host,
                same_origin(&initial, &current),
                options.user_agent.as_deref(),
            ) {
                request = request.header(name, value);
            }
        }

        let response = request.send().await.map_err(map_error)?;

        if let Some(jar) = &options.cookie_jar {
            let set_cookies: Vec<String> = response
                .headers()
                .get_all(wreq::header::SET_COOKIE)
                .iter()
                .filter_map(|value| value.to_str().ok())
                .map(str::to_owned)
                .collect();
            if !set_cookies.is_empty() {
                jar.store_set_cookies(&set_cookies, &current);
            }
        }

        let status = response.status();
        if options.follow_redirects && status.is_redirection() {
            let Some(location) = response
                .headers()
                .get(wreq::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
            else {
                // 3xx 但没有 Location：不是可跟随的重定向，按最终响应返回。
                return finish(response, current, hops, options, started).await;
            };
            let next = current.join(&location).map_err(|error| {
                TransportError::new(
                    ErrorCode::BadUrl,
                    format!("invalid redirect Location {location:?}: {error}"),
                )
            })?;
            hops.push(RedirectHop {
                url: current.to_string(),
                status: status.as_u16(),
                location: next.to_string(),
            });
            if visited.iter().any(|seen| seen == next.as_str()) {
                return Err(TransportError::new(
                    ErrorCode::Network,
                    format!(
                        "redirect loop detected after {} hop(s): {}",
                        hops.len(),
                        describe_chain(&hops)
                    ),
                ));
            }
            if hops.len() > options.max_redirects {
                return Err(TransportError::new(
                    ErrorCode::Network,
                    format!(
                        "redirect limit exceeded (max {} hop(s)): {}",
                        options.max_redirects,
                        describe_chain(&hops)
                    ),
                ));
            }
            visited.push(next.as_str().to_owned());
            current = next;
            continue;
        }

        if http_errors_are_failure && (status.is_client_error() || status.is_server_error()) {
            // HTTP-14：4xx/5xx 归类为 BLOCKED（拦截/拒绝），不与 PARSE 混淆。
            return Err(TransportError::new(
                ErrorCode::Blocked,
                format!(
                    "HTTP {} {} at {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("Unknown"),
                    current
                ),
            ));
        }

        return finish(response, current, hops, options, started).await;
    }
}

async fn finish(
    response: wreq::Response,
    final_url: Url,
    hops: Vec<RedirectHop>,
    options: &FetchOptions,
    started: Instant,
) -> Result<Fetched, TransportError> {
    let status = response.status().as_u16();
    let version = version_str(response.version());
    let declared_len = response.content_length();
    let headers = header_pairs(response.headers());
    let (bytes, truncated) = read_body(response, options.max_body_bytes).await?;
    let content_type = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.clone());
    let kind = content_type::classify(content_type.as_deref(), &bytes);
    let content = build_body(kind, content_type, &bytes, declared_len, truncated);
    let mut fetched = Fetched {
        status,
        version,
        final_url: final_url.to_string(),
        headers,
        redirects: hops,
        content,
        elapsed_ms: elapsed_ms(started),
    };
    // SEC-04：信封一旦离开取回层就不含 cookie / Authorization 明文。
    crate::redact::redact_fetched(&mut fetched);
    Ok(fetched)
}

/// 两个 URL 是否同源（scheme + host + 生效端口）；SEC-06 的判定依据。
pub(crate) fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str().map(str::to_ascii_lowercase)
            == right.host_str().map(str::to_ascii_lowercase)
        && left.port_or_known_default() == right.port_or_known_default()
}

/// 流式读取正文，命中上限即停止读取并丢弃连接（不把超大响应读进内存）。
async fn read_body(
    response: wreq::Response,
    max_body_bytes: usize,
) -> Result<(Vec<u8>, bool), TransportError> {
    let mut stream = response.bytes_stream();
    let mut body: Vec<u8> = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(map_error)?;
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

fn build_body(
    kind: ContentKind,
    content_type: Option<String>,
    bytes: &[u8],
    declared_len: Option<u64>,
    truncated: bool,
) -> Body {
    let (text, charset, charset_source, decode_had_errors) = if kind.is_text() {
        let decoded = encoding::decode_body(bytes, content_type.as_deref());
        (
            Some(decoded.text),
            Some(decoded.encoding),
            Some(decoded.source),
            decoded.had_errors,
        )
    } else {
        (None, None, None, false)
    };
    Body {
        kind,
        content_type,
        byte_len: bytes.len(),
        declared_len,
        truncated,
        text,
        charset,
        charset_source,
        decode_had_errors,
    }
}

fn build_client(options: &FetchOptions) -> Result<wreq::Client, TransportError> {
    let mut builder = wreq::Client::builder()
        .redirect(Policy::none())
        .timeout(options.timeout);
    builder = match &options.proxy {
        Some(proxy) => {
            let proxy = wreq::Proxy::all(proxy.as_str()).map_err(|error| {
                TransportError::new(ErrorCode::BadUrl, format!("invalid proxy url: {error}"))
            })?;
            builder.proxy(proxy)
        }
        // 无显式代理时固定直连，避免被宿主环境代理劫持（离线测试要求）。
        None => builder.no_proxy(),
    };
    if let Some(user_agent) = &options.user_agent {
        builder = builder.user_agent(user_agent.as_str());
    }
    builder.build().map_err(|error| {
        TransportError::new(ErrorCode::Internal, format!("client init failed: {error}"))
    })
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

fn version_str(version: wreq::Version) -> String {
    match version {
        wreq::Version::HTTP_09 => "http/0.9",
        wreq::Version::HTTP_10 => "http/1.0",
        wreq::Version::HTTP_11 => "http/1.1",
        wreq::Version::HTTP_2 => "http/2",
        wreq::Version::HTTP_3 => "http/3",
        _ => "unknown",
    }
    .to_owned()
}

fn describe_chain(hops: &[RedirectHop]) -> String {
    let mut parts: Vec<String> = hops.iter().map(|hop| hop.url.clone()).collect();
    if let Some(last) = hops.last() {
        parts.push(last.location.clone());
    }
    parts.join(" -> ")
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// 把 wreq 错误归一到封闭错误码。
fn map_error(error: wreq::Error) -> TransportError {
    if error.is_timeout() {
        return TransportError::new(ErrorCode::Timeout, format!("request timed out: {error}"));
    }
    if error.is_decode() {
        return TransportError::new(
            ErrorCode::Parse,
            format!("response body decode failed: {error}"),
        );
    }
    if error.is_connect() {
        return TransportError::new(ErrorCode::Network, format!("connection failed: {error}"));
    }
    TransportError::new(ErrorCode::Network, format!("transport error: {error}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TransportError {
    code: ErrorCode,
    message: String,
}

impl TransportError {
    fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
