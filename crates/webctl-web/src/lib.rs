//! webctl 内置层：取回 + 解析 + 会话 + 文档与数据（PLAN v0.4 §2）。
//!
//! PR 01 只建立 crate 骨架；PR 02 落地取回层（`transport` / `encoding` / `content_type`）。
//! 硬约束：零外部进程、零浏览器（PLAN v0.4 §3.1）。
//!
//! # feature `offline`（议题 #22）
//!
//! `cargo test --features offline` 是本 crate 的 L0 离线通道：全部测试只访问
//! `127.0.0.1` fixture 与本地文件，零外网、零子进程。该 feature **不是**网络放宽开关，
//! 只是离线通道的显式标记；默认门禁 `cargo test` 同样离线（见 `tests/offline_lane.rs`）。

pub mod content_type;
pub mod encoding;
pub mod transport;

pub use content_type::ContentKind;
pub use encoding::{CharsetSource, DecodedText, decode_body};
pub use transport::{
    Body, CookieJar, DEFAULT_MAX_BODY_BYTES, DEFAULT_MAX_REDIRECTS, DEFAULT_TIMEOUT, FetchOptions,
    Fetched, RedirectHop, StoredCookie, fetch,
};

/// crate 标识，供能力自检与诊断输出引用。
pub const CRATE: &str = "webctl-web";
