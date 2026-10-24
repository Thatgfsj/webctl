//! session 集成测试的共享辅助（临时 store / engine / 凭证）。
//!
//! 由 tests/session.rs 拆出，供 session 与 session_forms 两个测试二进制共用。

// 共享辅助：各二进制只用一部分。
#![allow(dead_code)]

use std::time::Duration;

use webctl_web::session::{FormFill, SessionEngine, SessionStore};

// ---------------------------------------------------------------------------
// 测试辅助
// ---------------------------------------------------------------------------

pub fn temp_store(label: &str) -> SessionStore {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("webctl-pr04-{label}-{nanos}"));
    SessionStore::with_ttl(dir, Duration::from_secs(3600))
}

pub fn engine(label: &str) -> SessionEngine {
    SessionEngine::with_store("default", temp_store(label)).expect("engine")
}

pub fn credentials() -> FormFill {
    let mut fill = FormFill::new();
    fill.set("username", "alice").set("password", "secret");
    fill
}

// ---------------------------------------------------------------------------
// SESS
// ---------------------------------------------------------------------------
