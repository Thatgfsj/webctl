//! golden 基线约定（议题 #22 §4；服务 ENV-07、PARSE-*、EXTRACT-*）。
//!
//! 约定：
//! - 基线文件放在 tests/golden/，JSON 用 .json、纯文本用 .txt；
//! - 默认测试只**比对**，不写文件；
//! - 有意更新：UPDATE_GOLDEN=1 cargo test --features offline --test golden_envelope
//!   然后人工审阅 git diff 再提交（详见 tests/golden/README.md）。
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// golden 目录。
pub fn golden_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// 是否处于"有意更新基线"模式。
pub fn update_requested() -> bool {
    std::env::var("UPDATE_GOLDEN").is_ok_and(|value| value == "1" || value == "true")
}

/// 比对文本基线；更新模式下写入并返回。
pub fn assert_text_golden(name: &str, actual: &str) {
    compare_or_update(&golden_root().join(format!("{name}.txt")), name, actual);
}

/// 比对 JSON 基线（美化输出后逐字比较）。
pub fn assert_json_golden(name: &str, value: &serde_json::Value) {
    let actual = serde_json::to_string_pretty(value).expect("serialize golden value");
    compare_or_update(&golden_root().join(format!("{name}.json")), name, &actual);
}

/// 读取基线文本；不存在则给出更新指引并失败。
pub fn read_golden(name: &str, extension: &str) -> String {
    let path = golden_root().join(format!("{name}.{extension}"));
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "缺少 golden 基线 {}: {error}\n用 UPDATE_GOLDEN=1 cargo test --features offline --test golden_envelope 生成",
            path.display()
        )
    })
}

fn compare_or_update(path: &Path, name: &str, actual: &str) {
    if update_requested() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create golden dir");
        }
        let normalized = normalize(actual);
        std::fs::write(path, &normalized).expect("write golden");
        eprintln!("golden 已更新：{}（审阅 diff 后再提交）", path.display());
        return;
    }
    let expected = std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!(
            "缺少 golden 基线 {}: {error}\n用 UPDATE_GOLDEN=1 cargo test --features offline --test golden_envelope 生成",
            path.display()
        )
    });
    assert_eq!(
        normalize(actual),
        normalize(&expected),
        "golden 不匹配（{name}）：{}\n如需有意更新：UPDATE_GOLDEN=1 cargo test --features offline --test golden_envelope",
        path.display()
    );
}

fn normalize(text: &str) -> String {
    let mut normalized = text.replace("\r\n", "\n");
    if !normalized.ends_with('\n') {
        normalized.push('\n');
    }
    normalized
}
