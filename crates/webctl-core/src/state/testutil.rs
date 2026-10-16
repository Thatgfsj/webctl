//! 测试用临时目录工具（仅 cfg(test)）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// 创建唯一的临时目录。
pub(crate) fn temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("webctl-core-state-{label}-{nanos}-{seq}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// 递归清理。
pub(crate) fn cleanup(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
}
