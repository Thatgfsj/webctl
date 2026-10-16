//! 协议级会话引擎（PLAN v0.4 §2「会话」域，PR 04）。
//!
//! 目标：没有浏览器也能完成“像用户一样”的表单流。
//! - 会话：cookie 持久化（复用 transport 的 CookieJar 快照）、会话标识与导入导出；
//! - 表单：字段类型映射、隐藏字段保留、CSRF 提取/回填（含轮换后重取）、multipart 上传；
//! - 分页：next 链接识别与遍历，有页数上限，循环 next 在上限内停止并标 LIMIT；
//! - 降级：识别 SPA/JS 依赖页面并显式 DEGRADED，未登录返回 SESSION_LOST。
//!
//! 边界（诚实声明）：只覆盖服务端渲染与表单流；没有 JS 引擎，SPA 一律降级，
//! 不伪造成功、不用启发式补全内容。

pub mod csrf;
pub mod engine;
pub mod form;
pub mod html;
pub mod pagination;
pub mod spa;
pub mod state;
pub mod store;

pub use csrf::{
    CsrfSource, CsrfToken, extract_form_tokens, extract_meta_tokens, extract_tokens, is_csrf_name,
    refill_from_html,
};
pub use engine::{DEFAULT_ITEM_ELEMENT, DEFAULT_MAX_PAGES, Page, Paginated, SessionEngine};
pub use form::{
    EncodedBody, Field, FieldKind, FileUpload, Form, FormEnctype, FormError, FormFill, FormMethod,
    SelectOption,
};
pub use pagination::{NextLink, NextSource, find_next};
pub use spa::{SpaSignal, detect as detect_spa};
pub use state::{
    DEFAULT_TTL, MAX_SESSION_NAME_LEN, SESSION_FILE_VERSION, SessionError, SessionState,
    validate_session_name,
};
pub use store::SessionStore;
