//! webctl 内置层：取回 + 解析 + 会话 + 文档与数据（PLAN v0.4 §2）。
//!
//! PR 01 只建立 crate 骨架；PR 02 落地取回层（`transport` / `encoding` / `content_type`）。
//! 硬约束：零外部进程、零浏览器（PLAN v0.4 §3.1）。

pub mod content_type;
pub mod doc;
pub mod encoding;
pub mod transport;

pub use content_type::ContentKind;
pub use doc::json_query::json_query;
pub use doc::{
    DEFAULT_MAX_DOC_BYTES, DEFAULT_MAX_JSON_BYTES, DocFormat, DocOptions, DocText,
    JsonQueryOptions, JsonQueryOutcome, PdfKind, doc_text, extract_pdf_text,
};
pub use encoding::{CharsetSource, DecodedText, decode_body};
pub use transport::{
    Body, CookieJar, DEFAULT_MAX_BODY_BYTES, DEFAULT_MAX_REDIRECTS, DEFAULT_TIMEOUT, FetchOptions,
    Fetched, RedirectHop, StoredCookie, fetch,
};

/// crate 标识，供能力自检与诊断输出引用。
pub const CRATE: &str = "webctl-web";
