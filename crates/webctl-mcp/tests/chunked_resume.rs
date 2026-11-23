//! #190 分块续读（extract / doc_text）的 MCP 验收。
//!
//! 全部离线：输入是 `html` 参数或临时目录里的纯文本文件；fetch 用例只连
//! 127.0.0.1 的进程内 HTTP fixture，不访问外网。

mod common;

use common::{http_fixture, in_process_context, temp_dir};
use serde_json::{Value, json};
use webctl_mcp::{TOOL_NAMES, WebctlMcp, tool_definitions};

fn server(tag: &str) -> WebctlMcp {
    WebctlMcp::new(in_process_context(tag))
}

async fn invoke(mcp: &WebctlMcp, name: &str, args: Value) -> Value {
    mcp.invoke(name, args)
        .await
        .unwrap_or_else(|| panic!("工具 {name} 未注册"))
}

/// 含中文、ASCII 与 emoji 的长文本，覆盖多字节字符边界。
fn long_text() -> String {
    let mut text = String::new();
    for index in 0..40 {
        text.push_str(&format!(
            "第{index:02}段：分块续读必须逐字节一致 — 😀🚀 0123456789 abcdefghijklmnopqrstuvwxyz
"
        ));
    }
    text
}

fn assert_detected_contains(envelope: &Value, needle: &str) {
    let detected = envelope["meta"]["actionable"]["detected"]
        .as_array()
        .unwrap_or_else(|| panic!("缺少 actionable.detected：{envelope}"));
    assert!(
        detected
            .iter()
            .any(|item| item.as_str().is_some_and(|text| text.contains(needle))),
        "actionable.detected 未说明 {needle}：{envelope}"
    );
}

/// #190：分块续读拼接后与一次性完整扁平结果逐字节一致。
#[tokio::test]
async fn extract_chunks_reassemble_to_the_full_flat_text() {
    let mcp = server("chunk-extract-full");
    let html = format!("<html><body><p>{}</p></body></html>", long_text());
    let full = invoke(
        &mcp,
        "extract",
        json!({"html": html, "selector": "p", "start_index": 0, "char_budget": 1_000_000}),
    )
    .await;
    assert_eq!(full["ok"], true, "{full}");
    assert_eq!(full["code"], "OK", "{full}");
    let full_text = full["data"]["text"]
        .as_str()
        .expect("完整调用必须返回 data.text")
        .to_owned();
    assert!(
        full_text.chars().count() > 500,
        "fixture 太小：{full_text:?}"
    );
    assert!(full["data"]["next_start_index"].is_null(), "{full}");

    let mut assembled = String::new();
    let mut start = 0usize;
    for _ in 0..1_000 {
        let chunk = invoke(
            &mcp,
            "extract",
            json!({"html": html, "selector": "p", "start_index": start, "char_budget": 31}),
        )
        .await;
        assert_eq!(chunk["ok"], true, "{chunk}");
        assembled.push_str(chunk["data"]["text"].as_str().expect("chunk text"));
        match chunk["data"]["next_start_index"].as_u64() {
            Some(next) => start = usize::try_from(next).expect("next fits"),
            None => break,
        }
    }
    assert_eq!(assembled.as_bytes(), full_text.as_bytes());
}

/// #190：start_index 落在多字节字符中间时向后对齐，绝不切出非法 UTF-8。
#[tokio::test]
async fn extract_start_index_mid_codepoint_advances_to_boundary() {
    let mcp = server("chunk-extract-snap");
    let html = "<html><body><p>A中B</p></body></html>";
    let full = invoke(
        &mcp,
        "extract",
        json!({"html": html, "selector": "p", "start_index": 0, "char_budget": 100}),
    )
    .await;
    assert_eq!(full["data"]["text"], "A中B", "{full}");

    // 字节 2 落在「中」(1..4) 的中间；对齐到字节 3 后应只剩 "B"。
    let snapped = invoke(
        &mcp,
        "extract",
        json!({"html": html, "selector": "p", "start_index": 2, "char_budget": 100}),
    )
    .await;
    assert_eq!(snapped["ok"], true, "{snapped}");
    assert_eq!(snapped["data"]["text"], "B", "{snapped}");
    assert_eq!(snapped["data"]["start_index"], 4, "{snapped}");
    assert!(snapped["data"]["next_start_index"].is_null(), "{snapped}");
}

/// #190：越界是 PARSE 失败并给出实际长度；恰好等于末尾是合法读完。
#[tokio::test]
async fn extract_start_index_out_of_range_is_parse_and_end_is_empty() {
    let mcp = server("chunk-extract-range");
    let html = "<html><body><p>A中B</p></body></html>";
    let rejected = invoke(
        &mcp,
        "extract",
        json!({"html": html, "selector": "p", "start_index": 999, "char_budget": 10}),
    )
    .await;
    assert_eq!(rejected["ok"], false, "{rejected}");
    assert_eq!(rejected["code"], "PARSE", "{rejected}");
    assert!(
        rejected["error"]
            .as_str()
            .is_some_and(|text| text.contains("超出扁平结果长度")),
        "{rejected}"
    );

    let full = invoke(
        &mcp,
        "extract",
        json!({"html": html, "selector": "p", "start_index": 0, "char_budget": 100}),
    )
    .await;
    let len = full["data"]["text"].as_str().expect("text").len();
    let end = invoke(
        &mcp,
        "extract",
        json!({"html": html, "selector": "p", "start_index": len, "char_budget": 10}),
    )
    .await;
    assert_eq!(end["ok"], true, "{end}");
    assert_eq!(end["data"]["text"], "", "{end}");
    assert!(end["data"]["next_start_index"].is_null(), "{end}");
}

/// #190：默认不传续读参数时行为不变（不新增 data.text），游标为 null。
#[tokio::test]
async fn extract_without_chunk_parameters_is_unchanged() {
    let mcp = server("chunk-extract-default");
    let envelope = invoke(
        &mcp,
        "extract",
        json!({"html": "<html><body><p>short</p></body></html>", "selector": "p"}),
    )
    .await;
    assert_eq!(envelope["code"], "OK", "{envelope}");
    assert!(
        envelope["data"].get("text").is_none(),
        "默认路径不得凭空多出 data.text：{envelope}"
    );
    assert!(envelope["data"]["next_start_index"].is_null(), "{envelope}");
    assert_eq!(
        envelope["data"]["matches"][0]["text"], "short",
        "{envelope}"
    );
}

/// #190：既有 char_budget 截断给出续读游标；actionable 明说可带 start_index。
#[tokio::test]
async fn extract_char_budget_truncation_exposes_resumable_cursor() {
    let mcp = server("chunk-extract-cursor");
    let html = format!("<html><body><p>{}</p></body></html>", long_text());
    let first = invoke(
        &mcp,
        "extract",
        json!({"html": html, "selector": "p", "char_budget": 20}),
    )
    .await;
    assert_eq!(first["code"], "LIMIT", "{first}");
    assert_eq!(
        first["meta"]["actionable"]["reason"], "result-budget-exhausted",
        "{first}"
    );
    assert_eq!(
        first["meta"]["actionable"]["next_steps"],
        json!(["extract", "doc_text"]),
        "{first}"
    );
    assert_detected_contains(&first, "resumable");
    let next = first["data"]["next_start_index"]
        .as_u64()
        .expect("char-budget 截断必须给出续读游标");
    let head = first["data"]["matches"][0]["text"]
        .as_str()
        .expect("第一条命中")
        .to_owned();

    let second = invoke(
        &mcp,
        "extract",
        json!({"html": html, "selector": "p", "start_index": next, "char_budget": 20}),
    )
    .await;
    assert_eq!(second["ok"], true, "{second}");
    let tail = second["data"]["text"].as_str().expect("第二块");
    let full = invoke(
        &mcp,
        "extract",
        json!({"html": html, "selector": "p", "start_index": 0, "char_budget": 1_000_000}),
    )
    .await;
    let full_text = full["data"]["text"].as_str().expect("完整文本");
    let expected: String = full_text.chars().take(40).collect();
    assert_eq!(
        format!("{head}{tail}"),
        expected,
        "两段拼接必须等于完整结果前缀"
    );
}

/// #190：doc_text 分块续读拼接后与一次性完整结果逐字节一致。
#[tokio::test]
async fn doc_text_chunks_reassemble_to_the_full_text() {
    let mcp = server("chunk-doc-full");
    let dir = temp_dir("chunk-doc-file");
    let path = dir.join("long.txt");
    let text = long_text();
    std::fs::write(&path, text.as_bytes()).expect("写 fixture");
    let path_arg = path.to_string_lossy().into_owned();

    let full = invoke(
        &mcp,
        "doc_text",
        json!({"path": path_arg, "start_index": 0, "char_budget": 1_000_000}),
    )
    .await;
    assert_eq!(full["code"], "OK", "{full}");
    assert_eq!(full["data"]["text"], text, "{full}");
    assert!(full["data"]["next_start_index"].is_null(), "{full}");

    let mut assembled = String::new();
    let mut start = 0usize;
    for _ in 0..1_000 {
        let chunk = invoke(
            &mcp,
            "doc_text",
            json!({"path": path_arg, "start_index": start, "char_budget": 29}),
        )
        .await;
        assert_eq!(chunk["ok"], true, "{chunk}");
        assembled.push_str(chunk["data"]["text"].as_str().expect("chunk text"));
        match chunk["data"]["next_start_index"].as_u64() {
            Some(next) => start = usize::try_from(next).expect("next fits"),
            None => break,
        }
    }
    assert_eq!(assembled.as_bytes(), text.as_bytes());
}

/// #190：doc_text 的越界 / 字符中间 / 末尾边界。
#[tokio::test]
async fn doc_text_start_index_boundaries_are_explicit() {
    let mcp = server("chunk-doc-boundary");
    let dir = temp_dir("chunk-doc-boundary-file");
    let path = dir.join("boundary.txt");
    std::fs::write(&path, "A中B").expect("写 fixture");
    let path_arg = path.to_string_lossy().into_owned();

    let snapped = invoke(
        &mcp,
        "doc_text",
        json!({"path": path_arg, "start_index": 2, "char_budget": 100}),
    )
    .await;
    assert_eq!(snapped["data"]["text"], "B", "{snapped}");
    assert_eq!(snapped["data"]["start_index"], 4, "{snapped}");

    let rejected = invoke(
        &mcp,
        "doc_text",
        json!({"path": path_arg, "start_index": 99, "char_budget": 10}),
    )
    .await;
    assert_eq!(rejected["ok"], false, "{rejected}");
    assert_eq!(rejected["code"], "PARSE", "{rejected}");
    assert!(
        rejected["error"]
            .as_str()
            .is_some_and(|text| text.contains("超出扁平结果长度")),
        "{rejected}"
    );

    let end = invoke(
        &mcp,
        "doc_text",
        json!({"path": path_arg, "start_index": 5, "char_budget": 10}),
    )
    .await;
    assert_eq!(end["ok"], true, "{end}");
    assert_eq!(end["data"]["text"], "", "{end}");
    assert!(end["data"]["next_start_index"].is_null(), "{end}");
}

/// #190：doc_text 默认不截断；只给 char_budget 时从 0 起返回第一块并标 LIMIT。
#[tokio::test]
async fn doc_text_char_budget_truncates_and_is_resumable() {
    let mcp = server("chunk-doc-budget");
    let dir = temp_dir("chunk-doc-budget-file");
    let path = dir.join("boundary.txt");
    std::fs::write(&path, "A中B").expect("写 fixture");
    let path_arg = path.to_string_lossy().into_owned();

    let plain = invoke(&mcp, "doc_text", json!({"path": path_arg})).await;
    assert_eq!(plain["code"], "OK", "{plain}");
    assert_eq!(plain["data"]["text"], "A中B", "{plain}");
    assert!(plain["data"]["next_start_index"].is_null(), "{plain}");

    let cut = invoke(
        &mcp,
        "doc_text",
        json!({"path": path_arg, "char_budget": 2}),
    )
    .await;
    assert_eq!(cut["code"], "LIMIT", "{cut}");
    assert_eq!(cut["data"]["text"], "A中", "{cut}");
    assert_eq!(cut["data"]["next_start_index"], 4, "{cut}");
    assert_eq!(
        cut["meta"]["actionable"]["reason"], "result-budget-exhausted",
        "{cut}"
    );
    assert_detected_contains(&cut, "resumable");
}

/// #190：工具面仍 18 个，且两个工具都声明了续读参数。
#[test]
fn chunk_parameters_are_declared_and_tool_surface_stays_18() {
    let definitions = tool_definitions();
    assert_eq!(definitions.len(), 18, "工具面必须仍是 18 个");
    assert_eq!(TOOL_NAMES.len(), 18);
    for (name, expected) in [
        ("extract", vec!["start_index"]),
        ("doc_text", vec!["start_index", "char_budget"]),
    ] {
        let tool = definitions
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("工具 {name} 必须在注册表里"));
        let schema = serde_json::to_value(tool).expect("tool schema 可序列化");
        for key in expected {
            assert!(
                schema["inputSchema"]["properties"].get(key).is_some(),
                "{name} 必须声明 {key}：{schema}"
            );
        }
    }
}

/// #190：fetch 的传输截断保留原 actionable 通道，但明确写不可续读。
#[tokio::test]
async fn fetch_truncation_reports_not_resumable() {
    let fixture =
        http_fixture::Server::start_with(vec![("/big", http_fixture::Response::large(4096, b'x'))])
            .await;
    let mcp = server("chunk-fetch");
    let value = invoke(
        &mcp,
        "fetch",
        json!({"url": fixture.url("/big"), "max_bytes": 256}),
    )
    .await;
    assert_eq!(value["code"], "LIMIT", "{value}");
    assert_eq!(
        value["meta"]["actionable"]["reason"], "response-truncated",
        "{value}"
    );
    assert_detected_contains(&value, "not resumable");
    assert!(
        value["data"].get("next_start_index").is_none(),
        "fetch 不得假装可续读：{value}"
    );
}
/// #187 × #190：markdown 视图与续读游标是两套坐标，组合必须在取回之前显式 `PARSE`。
#[tokio::test]
async fn markdown_view_rejects_resume_parameters() {
    let mcp = server("chunk-markdown-conflict");
    let html = "<html><body><h1>T</h1></body></html>";
    for (tool, args) in [
        (
            "extract",
            json!({"html": html, "selector": "h1", "format": "markdown", "start_index": 0}),
        ),
        (
            "doc_text",
            json!({"url": "http://127.0.0.1:1/never", "format": "markdown", "start_index": 0}),
        ),
        (
            "doc_text",
            json!({"url": "http://127.0.0.1:1/never", "format": "markdown", "char_budget": 8}),
        ),
    ] {
        let value = invoke(&mcp, tool, args).await;
        assert_eq!(value["code"], "PARSE", "{tool}: {value}");
        let error = value["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("format=markdown") && error.contains("互斥"),
            "{tool} 必须说明互斥原因：{error}"
        );
    }
}
