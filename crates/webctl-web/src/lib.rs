//! webctl 内置层：取回 + 解析 + 会话 + 文档与数据（PLAN v0.4 §2）。
//!
//! PR 01 只建立 crate 骨架；PR 02 落地取回层（`transport` / `encoding` / `content_type`）；
//! PR 03 落地解析层（`parse`）。
//! 硬约束：零外部进程、零外部浏览器（PLAN v0.4 §3.1 / ADR-0001；唯一的进程内 JS 引擎见 ADR-0005）。
//!
//! # feature `offline`（议题 #22）
//!
//! `cargo test --features offline` 是本 crate 的 L0 离线通道：全部测试只访问
//! `127.0.0.1` fixture 与本地文件，零外网、零子进程。该 feature **不是**网络放宽开关，
//! 只是离线通道的显式标记；默认门禁 `cargo test` 同样离线（见 `tests/offline_lane.rs`）。

pub mod content_type;
pub mod degrade;
pub mod doc;
pub mod encoding;
pub mod extract;
pub mod parse;
pub mod paths;
pub mod politeness;
pub mod redact;
pub mod session;
pub mod transport;

pub use content_type::ContentKind;
pub use degrade::{
    Assessment, DEGRADED_FROM, DegradeOptions, Diagnosis, Scope, Severity, assess, guard,
};
pub use doc::json_query::json_query;
pub use doc::{
    DEFAULT_MAX_DOC_BYTES, DEFAULT_MAX_JSON_BYTES, DocFormat, DocOptions, DocText,
    JsonQueryOptions, JsonQueryOutcome, PdfKind, doc_text, extract_pdf_text,
};
pub use encoding::{CharsetSource, DecodedText, decode_body};
pub use paths::{
    ALLOWED_DOWNLOAD_TYPES, DEFAULT_MAX_DOWNLOAD_BYTES, DownloadError, DownloadPolicy,
    commit_download, ensure_download_root,
};
pub use politeness::{PoliteFetched, PoliteFetcher, PolitenessConfig, RobotsOutcome};
pub use redact::{
    REDACTED, redact_fetched, redact_header_pairs, redact_header_value, redact_log_line,
    redact_session_state, redact_stored_cookies, redact_url,
};
pub use session::{Page, Paginated, SessionEngine, SessionState, SessionStore};
pub use transport::{
    Body, CookieJar, DEFAULT_MAX_BODY_BYTES, DEFAULT_MAX_REDIRECTS, DEFAULT_TIMEOUT, FetchOptions,
    Fetched, RedirectHop, StoredCookie, fetch,
};

/// crate 标识，供能力自检与诊断输出引用。
pub const CRATE: &str = "webctl-web";
