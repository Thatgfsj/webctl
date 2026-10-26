//! #124 合规：降级原因表是**单一来源**，且源码里每条 `degraded_from` 都已登记。
//!
//! 背景：#115 只给 JS 渲染一条路径接了 `meta.actionable`，复审在 #123 抓到"文档说得比实现多"。
//! #124 的收口方式是把"原因码 + 建议通道"集中到 `webctl_core::actionable`，由
//! `Envelope::degraded` 统一补；本文件的扫描用例防止有人新增降级路径却忘了登记。

use std::path::{Path, PathBuf};

use serde_json::json;
use webctl_core::{Envelope, ErrorCode, Trust, actionable};
use webctl_web::extract::{self, ExtractOptions};
use webctl_web::transport::{FetchOptions, fetch};

#[path = "../../webctl-web/tests/support/http_fixture.rs"]
mod http_fixture;

const QUOTES: &str =
    "<html><body><div class=\"quote\">a</div><div class=\"quote\">b</div></body></html>";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf()
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).expect("dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    files
}

/// 取 `Envelope::degraded(` 之后 400 字符窗口里的双引号字面量（朴素奇偶切分）。
fn literals(window: &str) -> Vec<String> {
    window
        .split('"')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, text)| text.to_owned())
        .collect()
}

/// 降级来源的形态：小写字母/数字，且含 `-` / `_` / `:` 之一（排除 "http" 这类普通词）。
fn looks_like_degraded_from(literal: &str) -> bool {
    literal.len() >= 4
        && literal.chars().all(|ch| {
            ch.is_ascii_lowercase()
                || ch.is_ascii_digit()
                || ch == '-'
                || ch == '_'
                || ch == ':'
                || ch == '.'
        })
        && (literal.contains('-') || literal.contains(':'))
}

/// 扫描：源码里出现的每个 `degraded_from` 都必须在 `actionable` 登记表里。
#[test]
fn every_degraded_from_in_source_is_registered() {
    let registered = actionable::registered();
    let reasons = actionable::reasons();
    let crates = repo_root().join("crates");
    let mut unknown: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&crates).expect("crates dir") {
        let src = entry.expect("entry").path().join("src");
        if !src.is_dir() {
            continue;
        }
        for file in walk(&src) {
            let text = std::fs::read_to_string(&file).expect("read source");
            let mut offset = 0usize;
            while let Some(position) = text[offset..].find("Envelope::degraded(") {
                let start = offset + position + "Envelope::degraded(".len();
                // 按字符截窗口：源码里有中文注释，按字节切会落在字符中间。
                let window: String = text[start..].chars().take(400).collect();
                for literal in literals(&window) {
                    if !looks_like_degraded_from(&literal) {
                        continue;
                    }
                    if registered.iter().any(|key| *key == literal)
                        || reasons.iter().any(|reason| *reason == literal)
                        || literal.starts_with("content-type:")
                        || literal.contains(',')
                    {
                        continue;
                    }
                    unknown.push(format!("{literal} @ {}", file.display()));
                }
                offset = start;
            }
        }
    }
    unknown.sort_unstable();
    unknown.dedup();
    assert!(
        unknown.is_empty(),
        "以下降级来源没有登记到 webctl_core::actionable（#124）：\n{}",
        unknown.join("\n")
    );
}

/// 传输截断：LIMIT 降级必须带 reason + 建议通道（download）。
#[tokio::test]
async fn fetch_truncation_carries_actionable() {
    let server =
        http_fixture::Server::start_with(vec![("/big", http_fixture::Response::large(4096, b'x'))])
            .await;
    let options = FetchOptions {
        max_body_bytes: 256,
        ..FetchOptions::default()
    };
    let envelope = fetch(&server.url("/big"), &options).await;
    assert_eq!(envelope.code, ErrorCode::Limit, "{}", envelope.error);
    let actionable = envelope.meta.actionable.expect("必须带 actionable");
    assert_eq!(actionable.reason, "response-truncated");
    assert_eq!(actionable.next_steps, vec!["download".to_owned()]);
}

/// 抽取命中上限：LIMIT 降级必须带 reason + 建议通道（paginate）。
#[test]
fn extract_max_results_carries_actionable() {
    let options = ExtractOptions {
        max_results: 1,
        ..ExtractOptions::default()
    };
    let envelope = extract::extract(QUOTES, "div.quote", &options);
    assert_eq!(envelope.code, ErrorCode::Limit, "{}", envelope.error);
    let actionable = envelope.meta.actionable.expect("必须带 actionable");
    assert_eq!(actionable.reason, "result-limit-reached");
    assert!(actionable.next_steps.contains(&"paginate".to_owned()));
}

/// 表格跨行列裁剪：降级必须带 actionable；没有别的通道时 next_steps 为空数组（不许编造）。
#[test]
fn table_span_clamp_carries_actionable() {
    // colspan 超过 MAX_SPAN 才会被裁剪（不是任意跨行列都降级）。
    let html = "<table><tr><td colspan=\"999999\">h</td></tr><tr><td>a</td></tr></table>";
    let envelope = extract::tables(html, &ExtractOptions::default());
    assert_eq!(envelope.code, ErrorCode::Limit, "{}", envelope.error);
    let actionable = envelope.meta.actionable.expect("必须带 actionable");
    assert_eq!(actionable.reason, "table-span-clamped");
    assert!(actionable.next_steps.is_empty(), "没有别的通道就不要编造");
}

/// 未降级的成功信封不得带 actionable（协议误用会被 validate 拦下）。
#[test]
fn ok_envelope_has_no_actionable() {
    let envelope = Envelope::ok(json!({}), Trust::TrustedLocal, 1);
    assert!(envelope.meta.actionable.is_none());
    assert_eq!(envelope.validate(), Ok(()));
}

/// 分页聚合失败（后续页 404）：`pagination-fetch-failed` 也必须带 actionable（#124）。
///
/// 复审指出的真实路径：前面页面都是干净 OK（没有可继承的 actionable），后续页 fetch 失败——
/// 这条信封以前不带任何 actionable，"所有降级都带"就成了过度宣称。
#[tokio::test]
async fn pagination_fetch_failure_carries_actionable() {
    let page1 = "<html><body><p>page one</p><a rel=\"next\" href=\"/page2\">next</a></body></html>";
    let server = http_fixture::Server::start_with(vec![
        ("/page1", http_fixture::Response::html(page1)),
        ("/page2", http_fixture::Response::status(404)),
    ])
    .await;
    let dir = std::env::temp_dir().join(format!("webctl-124-paginate-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let engine = webctl_web::session::SessionEngine::with_store(
        "pagination-124",
        webctl_web::session::SessionStore::new(dir),
    )
    .expect("engine");
    let envelope = engine.paginate(&server.url("/page1"), Some(3)).await;
    assert_eq!(envelope.code, ErrorCode::Degraded, "{}", envelope.error);
    assert_eq!(
        envelope.meta.degraded_from.as_deref(),
        Some("pagination-fetch-failed")
    );
    let actionable = envelope.meta.actionable.expect("必须带 actionable");
    assert_eq!(actionable.reason, "page-fetch-failed");
    assert!(actionable.next_steps.contains(&"session".to_owned()));
}
