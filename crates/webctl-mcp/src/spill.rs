//! 大结果落盘（MCP-11）：超过内联上限的信封写入 state dir，只把路径返回给模型。
//!
//! 硬规则：落盘**不改变**信封的 code 与 meta——降级信封落盘后仍是降级（保留 degraded_from），
//! 绝不因为“换个地方放”就把 LIMIT 洗成 OK。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// 内联上限：超过就把完整信封落盘（32 KiB，与 SKILL.md 声明一致）。
pub const MAX_INLINE_BYTES: usize = 32 * 1024;

/// 落盘子目录（相对 state dir）。
pub const SPILL_SUBDIR: &str = "results";

/// 落盘目录。
pub fn spill_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(SPILL_SUBDIR)
}

/// 把渲染好的信封 JSON 变成要写进 MCP 结果的 Value。
///
/// - 不超过上限：原样解析返回（内联）。
/// - 超过上限且 ok=true：写入 results/ 并返回“引用信封”，保留原 code/meta。
/// - 超过上限但 ok=false：不会发生（失败信封不含 data），保险起见原样返回。
pub fn ingest(state_dir: &Path, tool: &str, rendered: &str) -> Result<Value, String> {
    if rendered.len() <= MAX_INLINE_BYTES {
        return serde_json::from_str(rendered)
            .map_err(|error| format!("invalid envelope json: {error}"));
    }
    let original: Value = serde_json::from_str(rendered)
        .map_err(|error| format!("invalid envelope json: {error}"))?;
    if original.get("ok") != Some(&Value::Bool(true)) {
        return Ok(original);
    }
    let code = original.get("code").cloned().unwrap_or_else(|| json!("OK"));
    let meta = original.get("meta").cloned().unwrap_or(Value::Null);
    let path = write_spill(state_dir, tool, rendered).map_err(|error| error.to_string())?;
    let data = json!({
        "inline": false,
        "tool": tool,
        "saved_to": path.to_string_lossy(),
        "bytes": rendered.len(),
        "note": "结果超过内联上限，完整信封已落盘；请读取 saved_to 指向的文件。"
    });
    Ok(json!({ "ok": true, "data": data, "error": "", "code": code, "meta": meta }))
}

fn write_spill(state_dir: &Path, tool: &str, rendered: &str) -> std::io::Result<PathBuf> {
    let dir = spill_dir(state_dir);
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let path = dir.join(format!("{tool}-{}-{nanos}.json", std::process::id()));
    std::fs::write(&path, rendered)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_state(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("webctl-mcp-spill-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn small_result_stays_inline() {
        let dir = temp_state("small");
        let rendered = json!({"ok": true, "data": {"a": 1}, "error": "", "code": "OK", "meta": {}})
            .to_string();
        let value = ingest(&dir, "caps", &rendered).expect("ingest");
        assert_eq!(value["data"]["a"], 1);
        assert!(!spill_dir(&dir).exists(), "小结果不得落盘");
    }

    #[test]
    fn large_result_is_spilled_and_keeps_degradation() {
        let dir = temp_state("large");
        let big = "x".repeat(MAX_INLINE_BYTES);
        let rendered = json!({
            "ok": true,
            "data": {"text": big},
            "error": "",
            "code": "LIMIT",
            "meta": {"engine": "builtin", "trust": "untrusted_page_content", "degraded_from": "response-body-truncated", "stale": false, "elapsed_ms": 1}
        })
        .to_string();
        let value = ingest(&dir, "fetch", &rendered).expect("ingest");
        assert_eq!(value["data"]["inline"], false);
        assert_eq!(value["code"], "LIMIT", "落盘不得把降级洗成 OK");
        assert_eq!(value["meta"]["degraded_from"], "response-body-truncated");
        let saved = value["data"]["saved_to"].as_str().expect("saved_to");
        let written = std::fs::read_to_string(saved).expect("spill file");
        assert_eq!(written, rendered);
    }
}
