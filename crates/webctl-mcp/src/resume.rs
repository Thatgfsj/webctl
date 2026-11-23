//! #190 分块续读：`extract` / `doc_text` 的 `start_index` 与 `next_start_index`。
//!
//! ## 扁平结果
//!
//! - `doc_text`：`data.text`。
//! - `extract`：`data.matches[].text` 按文档序用 `\n` 连接；只有 `fields` 时，
//!   按字段名序连接各字段的字符串值（数组逐项、null 跳过）。
//!
//! `start_index` 是**字节偏移**（UTF-8）。落在多字节字符中间时向后对齐到
//! 下一个字符边界，绝不切出非法 UTF-8；被跳过的半个字符不返回。
//! `char_budget` 按 Unicode 字符计数（与既有语义一致），且至少前进 1 个字符，
//! 避免 `start_index` 不动导致调用方死循环。
//!
//! ## 边界
//!
//! - `start_index == 扁平结果字节长度`：返回空串 + `next_start_index=null`（合法读完）。
//! - `start_index > 长度`：`PARSE` 失败并给出实际长度——不静默夹到末尾，
//!   因为越界通常意味着游标已过期（页面内容变了）。
//! - 截断（`next_start_index` 非 null）：保留 `LIMIT` 语义，`degraded_from`
//!   追加 `char-budget`，`meta.actionable.detected` 说明可带 `start_index` 续读。
//!
//! 只在 MCP 层做窗口切片：`extract` 的既有抽取预算/语义保持不变，续读模式
//! 才把抽取阶段的 `char_budget` 放开（否则第二次调用会在同一位置再次截断）。

use serde_json::{Value, json};
use webctl_core::{Envelope, ErrorCode};

/// `extract` 只给 `start_index` 时的默认分块大小（与既有 `char_budget` 默认一致）。
pub(crate) const DEFAULT_CHUNK_CHARS: usize = 8_000;

/// 一次切片结果。
#[derive(Debug)]
struct Chunk {
    /// 对齐后的起始字节（≥ 调用方传入的 start_index）。
    begin: usize,
    /// 结束字节（UTF-8 字符边界）。
    end: usize,
    /// 切片文本。
    text: String,
    /// 下一次的起始字节；到末尾为 None。
    next: Option<usize>,
}

/// 按「字节起始 + 字符预算」切一段扁平文本。
fn chunk_text(text: &str, start: usize, budget_chars: usize) -> Result<Chunk, String> {
    if start > text.len() {
        return Err(format!(
            "start_index {start} 超出扁平结果长度 {} 字节（UTF-8 字节偏移）",
            text.len()
        ));
    }
    let begin = snap_forward(text, start);
    let budget = budget_chars.max(1);
    let begin_chars = text[..begin].chars().count();
    let end = byte_at_char(text, begin_chars.saturating_add(budget));
    Ok(Chunk {
        begin,
        end,
        text: text[begin..end].to_owned(),
        next: (end < text.len()).then_some(end),
    })
}

/// 把落在多字节字符中间的字节偏移向后对齐到下一个字符边界。
fn snap_forward(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

/// 第 n 个 Unicode 字符的字节偏移；超出末尾返回文本长度。
fn byte_at_char(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

/// `extract` 的扁平结果：matches 优先，其次 fields。
fn flat_extract(data: &Value) -> String {
    if let Some(matches) = data.get("matches").and_then(Value::as_array) {
        let mut out = String::new();
        for (index, item) in matches.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(item.get("text").and_then(Value::as_str).unwrap_or(""));
        }
        return out;
    }
    let mut parts = Vec::new();
    if let Some(fields) = data.get("fields").and_then(Value::as_object) {
        for value in fields.values() {
            push_field_text(value, &mut parts);
        }
    }
    parts.join("\n")
}

fn push_field_text(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => out.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                push_field_text(item, out);
            }
        }
        Value::Null => {}
        other => out.push(other.to_string()),
    }
}

fn char_budget_truncated(data: &Value) -> bool {
    data.get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && data
            .get("truncated_reasons")
            .and_then(Value::as_array)
            .is_some_and(|reasons| {
                reasons
                    .iter()
                    .any(|reason| reason.as_str() == Some("char-budget"))
            })
}

/// `extract` 的分块续读入口。
///
/// - `start_index` 为 None：保持既有输出，只补 `next_start_index`（char_budget
///   截断时为已返回扁平文本的字节长度，否则 null）；
/// - `start_index` 为 Some：进入续读模式，返回从该字节开始的 `data.text` 窗口，
///   并把 `matches` 裁剪到同一窗口（`index` 保留原文档序）。
pub(crate) fn chunk_extract(
    mut envelope: Value,
    start: Option<usize>,
    budget: Option<usize>,
) -> Value {
    if envelope.get("ok").and_then(Value::as_bool) != Some(true) {
        return envelope;
    }
    let Some(flat) = envelope.get("data").map(flat_extract) else {
        return envelope;
    };

    let Some(start) = start else {
        let next = envelope
            .get("data")
            .is_some_and(char_budget_truncated)
            .then_some(flat.len());
        if let Some(data) = envelope.get_mut("data").filter(|data| data.is_object()) {
            data["next_start_index"] = json!(next);
        }
        return envelope;
    };

    let chunk = match chunk_text(&flat, start, budget.unwrap_or(DEFAULT_CHUNK_CHARS)) {
        Ok(chunk) => chunk,
        Err(message) => return fail_from(&envelope, message),
    };
    let cut = chunk.next.is_some();
    let (begin, end, next) = (chunk.begin, chunk.end, chunk.next);
    let chars = chunk.text.chars().count();
    if let Some(data) = envelope.get_mut("data").filter(|data| data.is_object()) {
        trim_matches(data, &flat, begin, end);
        data["text"] = json!(chunk.text);
        data["start_index"] = json!(begin);
        data["next_start_index"] = json!(next);
        data["conclusion"] = json!(if cut {
            format!(
                "resumed at byte {begin}; returned {chars} char(s) of the flattened result; continue with start_index={end}"
            )
        } else {
            format!("resumed at byte {begin}; returned the remaining flattened result")
        });
        if cut {
            data["truncated"] = json!(true);
            data["more_matches"] = json!(true);
            push_reason(data, "char-budget");
        }
    }
    if cut {
        mark_limit(&mut envelope);
    }
    envelope
}

/// `doc_text` 的分块续读入口。
///
/// - 两个参数都不给：保持既有输出，只补 `next_start_index=null`；
/// - 给 `start_index` 或 `char_budget`：从 `start_index`（默认 0）按字符预算切片；
///   只有 `char_budget` 也等价于「从 0 开始的第一块」。
pub(crate) fn chunk_doc_text(
    mut envelope: Value,
    start: Option<usize>,
    budget: Option<usize>,
) -> Value {
    if envelope.get("ok").and_then(Value::as_bool) != Some(true) {
        return envelope;
    }
    if start.is_none() && budget.is_none() {
        if let Some(data) = envelope.get_mut("data").filter(|data| data.is_object()) {
            data["next_start_index"] = Value::Null;
        }
        return envelope;
    }
    let Some(text) = envelope
        .get("data")
        .and_then(|data| data.get("text"))
        .and_then(Value::as_str)
    else {
        // 不支持/无文本层的降级结果：没有可续读的文本，但字段仍存在（null）。
        if let Some(data) = envelope.get_mut("data").filter(|data| data.is_object()) {
            data["next_start_index"] = Value::Null;
        }
        return envelope;
    };
    let text = text.to_owned();
    let chunk = match chunk_text(&text, start.unwrap_or(0), budget.unwrap_or(usize::MAX)) {
        Ok(chunk) => chunk,
        Err(message) => return fail_from(&envelope, message),
    };
    let cut = chunk.next.is_some();
    let (begin, end, next) = (chunk.begin, chunk.end, chunk.next);
    let chars = chunk.text.chars().count();
    if let Some(data) = envelope.get_mut("data").filter(|data| data.is_object()) {
        data["text"] = json!(chunk.text);
        data["start_index"] = json!(begin);
        data["next_start_index"] = json!(next);
        if cut && let Some(diagnostics) = data.get_mut("diagnostics").and_then(Value::as_array_mut)
        {
            diagnostics.push(json!(format!(
                "文本从字节 {begin} 起返回 {chars} 个字符，可带 start_index={end} 续读"
            )));
        }
    }
    if cut {
        mark_limit(&mut envelope);
    }
    envelope
}

/// 把 `matches` 裁到 `[begin, end)` 窗口：只保留与窗口相交的文本，边界处标记 truncated。
fn trim_matches(data: &mut Value, flat: &str, begin: usize, end: usize) {
    let Some(matches) = data.get_mut("matches").and_then(Value::as_array_mut) else {
        return;
    };
    let mut cursor = 0usize;
    let mut kept = Vec::new();
    for item in std::mem::take(matches) {
        let text = item
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let match_start = cursor;
        let match_end = match_start + text.len();
        cursor = match_end + 1; // 扁平时相邻 match 之间有一个 '\n'
        let lo = match_start.max(begin);
        let hi = match_end.min(end);
        if lo >= hi {
            continue;
        }
        let partial = lo != match_start || hi != match_end;
        let mut item = item;
        item["text"] = json!(flat[lo..hi]);
        if partial {
            item["text_truncated"] = json!(true);
            item["before"] = json!("");
            item["after"] = json!("");
        }
        kept.push(item);
    }
    *matches = kept;
}

fn push_reason(data: &mut Value, reason: &str) {
    if let Some(reasons) = data
        .get_mut("truncated_reasons")
        .and_then(Value::as_array_mut)
    {
        if !reasons.iter().any(|item| item.as_str() == Some(reason)) {
            reasons.push(json!(reason));
        }
    } else {
        data["truncated_reasons"] = json!([reason]);
    }
}

/// 把信封升级为 LIMIT 降级：追加 `char-budget` 来源并补可续读的 actionable。
fn mark_limit(envelope: &mut Value) {
    let Ok(mut typed) = serde_json::from_value::<Envelope<Value>>(envelope.clone()) else {
        return;
    };
    let combined = combine_reason(typed.meta.degraded_from.take(), "char-budget");
    typed.meta.degraded_from = Some(combined.clone());
    typed.code = ErrorCode::Limit;
    typed = webctl_core::actionable::attach(typed);
    if let Some(actionable) = typed.meta.actionable.as_mut() {
        // 当前降级原因是窗口截断：reason/next_steps 必须指向续读，而不是上游的
        // 渲染/命中数原因；detected 与 fallback_hint 保留上游证据。
        if let Some(plan) = webctl_core::actionable::plan_for("char-budget") {
            actionable.reason = plan.reason.to_owned();
            actionable.next_steps = plan
                .next_steps
                .iter()
                .map(|step| (*step).to_owned())
                .collect();
        }
        if let Some(note) = webctl_core::actionable::note_for(&combined)
            && !actionable.detected.iter().any(|item| item == note)
        {
            actionable.detected.push(note.to_owned());
        }
    }
    if let Ok(value) = serde_json::to_value(typed) {
        *envelope = value;
    }
}

fn combine_reason(existing: Option<String>, add: &str) -> String {
    match existing {
        Some(from) if from.split([',', '+']).any(|token| token.trim() == add) => from,
        Some(from) if !from.is_empty() => format!("{from},{add}"),
        _ => add.to_owned(),
    }
}

/// 用失败信封替换：保留原调用的耗时，错误信息说明为何不能续读。
fn fail_from(envelope: &Value, message: String) -> Value {
    let elapsed = envelope
        .get("meta")
        .and_then(|meta| meta.get("elapsed_ms"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    serde_json::to_value(Envelope::<()>::fail(ErrorCode::Parse, message, elapsed))
        .unwrap_or_else(|_| json!({"ok": false, "data": null, "error": "start_index 非法", "code": "PARSE", "meta": null}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_aligns_forward_and_never_splits_utf8() {
        let text = "A中B"; // A=1 字节，中=3 字节，B=1 字节。
        let chunk = chunk_text(text, 2, 10).expect("mid-char start must snap forward");
        assert_eq!(chunk.begin, 4);
        assert_eq!(chunk.text, "B");
        assert_eq!(chunk.next, None);
    }

    #[test]
    fn chunk_budget_counts_chars_not_bytes() {
        let text = "中中中中"; // 12 字节 / 4 字符
        let chunk = chunk_text(text, 0, 2).expect("chunk");
        assert_eq!(chunk.text, "中中");
        assert_eq!(chunk.next, Some(6));
        let rest = chunk_text(text, chunk.next.expect("next"), 2).expect("rest");
        assert_eq!(format!("{}{}", chunk.text, rest.text), text);
        assert_eq!(rest.text, "中中");
        assert_eq!(rest.next, None);
    }

    #[test]
    fn chunk_out_of_range_is_explicit() {
        let error = chunk_text("abc", 4, 1).expect_err("out of range");
        assert!(error.contains("超出扁平结果长度 3 字节"), "{error}");
    }

    #[test]
    fn chunk_at_end_is_empty_and_done() {
        let chunk = chunk_text("abc", 3, 8).expect("end");
        assert_eq!(chunk.text, "");
        assert_eq!(chunk.next, None);
    }

    #[test]
    fn chunk_zero_budget_still_advances_one_char() {
        let chunk = chunk_text("abc", 0, 0).expect("zero budget");
        assert_eq!(chunk.text, "a");
        assert_eq!(chunk.next, Some(1));
    }

    #[test]
    fn flat_extract_prefers_matches_then_fields() {
        let value = json!({"matches": [{"text": "a"}, {"text": "b"}]});
        assert_eq!(flat_extract(&value), "a\nb");
        let value = json!({"fields": {"a": "x", "b": ["y", null]}});
        assert_eq!(flat_extract(&value), "x\ny");
    }
}
