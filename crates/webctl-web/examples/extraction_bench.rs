//! 抽取质量基准：段落级 precision / recall / F1（议题 #189）。
//!
//! **开发工具**，不在默认 CI 矩阵；只评 HTML 正文抽取，不评价反爬/JS 渲染。
//! 完整口径、样本与实测见 `docs/EXTRACTION-BENCHMARK.md`。要点：
//!
//! - 一行一段；token = 拉丁/数字连续串或单个 CJK 字符；段 < `--min-tokens` 两侧不计；
//! - 段落间多重集 Dice ≥ `--threshold` 才可配对，按 Dice 降序贪心一对一（确定性）；
//! - 标题行（与 `<title>`/`<h1>` token 互为子集）不计分；
//! - 默认打 `Article.markdown`；`--view text` 量化 html2text 视图的损失。
//!
//! ```text
//! cargo run --release --example extraction_bench -- --report /tmp/webctl-extraction-bench/webctl.json
//! cargo run --release --example extraction_bench -- --view text --report /tmp/webctl-extraction-bench/webctl-text.json
//! cargo run --release --example extraction_bench -- --predictions DIR --report /tmp/webctl-extraction-bench/external.json
//! cargo run --release --example extraction_bench -- --dump news-001
//! ```
//!
//! 一键流程见 `scripts/extraction-benchmark.sh`；零网络、零子进程（只读本地文件 + 调库）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};
use webctl_web::parse::{extract_article, html};

/// fixtures 根目录（相对本 crate，保证从任意 cwd 运行都可复现）。
const DEFAULT_FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/extraction_bench"
);
/// 报告默认落盘目录：仓库外，避免产物入库。
const DEFAULT_OUT_DIR: &str = "/tmp/webctl-extraction-bench";
/// 匹配阈值默认值：段落级 Dice ≥ 0.6 才认为“同一段”。
const DEFAULT_THRESHOLD: f64 = 0.6;
/// 段落最小 token 数默认值：短导航/标题/标签不计入，两侧同口径。
const DEFAULT_MIN_TOKENS: usize = 4;
/// 报告里每条失败样例保留的最大字符数，避免 JSON 膨胀。
const SNIPPET_CHARS: usize = 160;

const USAGE: &str = "\
用法: extraction_bench [选项]
  --fixtures <DIR>       fixtures 目录（含 manifest.json；默认本 crate 的 tests/fixtures/extraction_bench）
  --report <PATH>        报告 JSON（默认 /tmp/webctl-extraction-bench/<webctl|external-*>.json）
  --predictions <DIR>    不跑 webctl，改为给 <DIR>/<id>.txt 的外部纯文本打分
  --view <markdown|text> 打分视图（默认 markdown）
  --threshold <F>        段落配对 Dice 阈值 0.0..=1.0（默认 0.6）
  --min-tokens <N>       段落最少 token 数（默认 4）
  --dump <ID>            只打印该样本逐段匹配，不写报告（-h 显示本帮助）
退出码: 0 成功；2 参数/输入错误。不联网、不启动子进程。";

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("extraction_bench: {message}");
            std::process::ExitCode::from(2)
        }
    }
}

/// webctl 的两个输出视图；段落级评测默认用结构保真的 markdown。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Markdown,
    Text,
}

impl View {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "markdown" => Ok(Self::Markdown),
            "text" => Ok(Self::Text),
            other => Err(format!("--view 只支持 markdown/text，实际: {other}")),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Text => "text",
        }
    }
}

/// 一次运行的配置。
struct Options {
    fixtures: PathBuf,
    report: PathBuf,
    predictions: Option<PathBuf>,
    view: View,
    threshold: f64,
    min_tokens: usize,
    dump: Option<String>,
}

/// 样本清单（`manifest.json`）：顺序即报告顺序，类别用于分组统计。
#[derive(Debug, Deserialize)]
struct Manifest {
    schema: u32,
    samples: Vec<SampleSpec>,
}

/// 单个样本：HTML 与参考纯文本都在 fixtures 目录内。
#[derive(Debug, Deserialize)]
struct SampleSpec {
    id: String,
    category: String,
    html: String,
    expected: String,
    #[serde(default)]
    note: String,
}

/// 配对计数：P/R/F1 的唯一输入。
#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    matched: usize,
    predicted: usize,
    reference: usize,
}

impl Counts {
    fn add(&mut self, other: Self) {
        self.matched += other.matched;
        self.predicted += other.predicted;
        self.reference += other.reference;
    }

    fn precision(self) -> f64 {
        if self.predicted == 0 {
            0.0
        } else {
            self.matched as f64 / self.predicted as f64
        }
    }

    fn recall(self) -> f64 {
        if self.reference == 0 {
            0.0
        } else {
            self.matched as f64 / self.reference as f64
        }
    }

    fn f1(self) -> f64 {
        let (p, r) = (self.precision(), self.recall());
        if p + r == 0.0 {
            0.0
        } else {
            2.0 * p * r / (p + r)
        }
    }
}

/// 一个样本的打分结果（含失败样例片段，便于报告里定位问题）。
struct SampleScore {
    counts: Counts,
    missed: Vec<String>,
    spurious: Vec<String>,
}

/// 一次 webctl 抽取的可观测结果（报告里要能看出是否走了降级/失败）。
struct WebctlOutcome {
    content: String,
    extractor: String,
    code: String,
    degraded_from: Option<String>,
    error: String,
}

fn run() -> Result<(), String> {
    let Some(options) = parse_options(std::env::args().skip(1))? else {
        println!("{USAGE}");
        return Ok(());
    };
    let manifest = load_manifest(&options.fixtures)?;
    if let Some(id) = options.dump.as_deref() {
        return dump_sample(&options, &manifest, id);
    }
    let rows = collect_rows(&options, &manifest)?;
    let report = build_report(&options, &manifest, &rows)?;
    write_report(&options.report, &report)?;
    print_table(&rows, &report);
    println!();
    println!("报告: {}", options.report.display());
    Ok(())
}

fn parse_options(args: impl Iterator<Item = String>) -> Result<Option<Options>, String> {
    let mut fixtures = PathBuf::from(DEFAULT_FIXTURES);
    let mut report: Option<PathBuf> = None;
    let mut predictions: Option<PathBuf> = None;
    let mut view = View::Markdown;
    let mut threshold = DEFAULT_THRESHOLD;
    let mut min_tokens = DEFAULT_MIN_TOKENS;
    let mut dump: Option<String> = None;
    let mut args = args.peekable();
    while let Some(argument) = args.next() {
        let mut value = || -> Result<String, String> {
            args.next().ok_or_else(|| format!("{argument} 缺少取值"))
        };
        match argument.as_str() {
            "-h" | "--help" => return Ok(None),
            "--fixtures" => fixtures = PathBuf::from(value()?),
            "--report" => report = Some(PathBuf::from(value()?)),
            "--predictions" => predictions = Some(PathBuf::from(value()?)),
            "--view" => view = View::parse(&value()?)?,
            "--threshold" => {
                let raw = value()?;
                threshold = raw
                    .parse::<f64>()
                    .map_err(|_| format!("--threshold 不是数字: {raw}"))?;
                if !(0.0..=1.0).contains(&threshold) {
                    return Err(format!("--threshold 必须在 0.0..=1.0: {threshold}"));
                }
            }
            "--min-tokens" => {
                let raw = value()?;
                min_tokens = raw
                    .parse::<usize>()
                    .map_err(|_| format!("--min-tokens 不是正整数: {raw}"))?;
                if min_tokens == 0 {
                    return Err("--min-tokens 必须 ≥ 1".to_owned());
                }
            }
            "--dump" => dump = Some(value()?),
            other => return Err(format!("未知参数 {other}（--help 查看用法）")),
        }
    }
    let report = report.unwrap_or_else(|| default_report_path(predictions.as_deref(), view));
    Ok(Some(Options {
        fixtures,
        report,
        predictions,
        view,
        threshold,
        min_tokens,
        dump,
    }))
}

/// 默认报告路径随模式/视图变化：webctl 基线固定 `webctl.json`，外部预测用目录名区分。
fn default_report_path(predictions: Option<&Path>, view: View) -> PathBuf {
    let out = Path::new(DEFAULT_OUT_DIR);
    match predictions {
        None if view == View::Markdown => out.join("webctl.json"),
        None => out.join(format!("webctl-{}.json", view.label())),
        Some(dir) => {
            let name = dir
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "external".to_owned());
            out.join(format!("external-{name}.json"))
        }
    }
}

fn load_manifest(fixtures: &Path) -> Result<Manifest, String> {
    let path = fixtures.join("manifest.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|error| format!("读不到样本清单 {}: {error}", path.display()))?;
    let manifest: Manifest = serde_json::from_str(&raw)
        .map_err(|error| format!("样本清单不是合法 JSON {}: {error}", path.display()))?;
    if manifest.schema != 1 {
        return Err(format!(
            "样本清单 schema 必须为 1，实际为 {}（{}）",
            manifest.schema,
            path.display()
        ));
    }
    if manifest.samples.is_empty() {
        return Err(format!("样本清单为空: {}", path.display()));
    }
    let mut seen = HashMap::new();
    for sample in &manifest.samples {
        if seen.insert(sample.id.clone(), ()).is_some() {
            return Err(format!("样本 id 重复: {}", sample.id));
        }
        for file in [&sample.html, &sample.expected] {
            let full = fixtures.join(file);
            if !full.is_file() {
                return Err(format!("样本 {} 缺少文件: {}", sample.id, full.display()));
            }
        }
    }
    Ok(manifest)
}

/// 收集每个样本的预测文本与抽取器状态，并立即打分。
fn collect_rows(options: &Options, manifest: &Manifest) -> Result<Vec<Value>, String> {
    let mut rows = Vec::with_capacity(manifest.samples.len());
    for sample in &manifest.samples {
        let reference_text = read_fixture(&options.fixtures, &sample.expected)?;
        let html_text = read_fixture(&options.fixtures, &sample.html)?;
        let title_tokens = document_title_tokens(&html_text);
        let (predicted_text, outcome) = match &options.predictions {
            Some(dir) => (read_prediction(dir, &sample.id)?, None),
            None => {
                let outcome = extract_with_webctl(&html_text, options.view);
                (outcome.content.clone(), Some(outcome))
            }
        };
        let predicted_text = drop_title_line(&predicted_text, &title_tokens);
        let reference = split_paragraphs(&reference_text);
        let predicted = split_paragraphs(&predicted_text);
        let score = score(
            &reference,
            &predicted,
            options.threshold,
            options.min_tokens,
        );
        let counts = score.counts;
        let (extractor, code, degraded_from, error) = match &outcome {
            Some(outcome) => (
                outcome.extractor.clone(),
                outcome.code.clone(),
                outcome.degraded_from.clone(),
                outcome.error.clone(),
            ),
            None => ("external".to_owned(), "OK".to_owned(), None, String::new()),
        };
        rows.push(json!({
            "id": sample.id,
            "category": sample.category,
            "note": sample.note,
            "extractor": extractor,
            "code": code,
            "degraded_from": degraded_from,
            "error": error,
            "reference_paragraphs": counts.reference,
            "predicted_paragraphs": counts.predicted,
            "matched": counts.matched,
            "precision": round4(counts.precision()),
            "recall": round4(counts.recall()),
            "f1": round4(counts.f1()),
            "missed": score.missed,
            "spurious": score.spurious,
        }));
    }
    Ok(rows)
}

fn read_fixture(fixtures: &Path, file: &str) -> Result<String, String> {
    let path = fixtures.join(file);
    std::fs::read_to_string(&path).map_err(|error| format!("读不到 {}: {error}", path.display()))
}

fn read_prediction(dir: &Path, id: &str) -> Result<String, String> {
    let path = dir.join(format!("{id}.txt"));
    std::fs::read_to_string(&path).map_err(|error| {
        format!(
            "外部预测缺少样本 {} 的文本 {}（每个样本一个 <id>.txt）: {error}",
            id,
            path.display()
        )
    })
}

/// 直接调用库函数（进程内，无网络、无子进程）。
fn extract_with_webctl(html: &str, view: View) -> WebctlOutcome {
    let envelope = extract_article(html, None);
    match envelope.data {
        Some(article) => {
            let content = match view {
                View::Markdown => article.markdown,
                View::Text => article.text,
            };
            WebctlOutcome {
                content,
                extractor: article.extractor,
                code: envelope.code.as_str().to_owned(),
                degraded_from: envelope.meta.degraded_from,
                error: String::new(),
            }
        }
        None => WebctlOutcome {
            content: String::new(),
            extractor: "none".to_owned(),
            code: envelope.code.as_str().to_owned(),
            degraded_from: envelope.meta.degraded_from,
            error: envelope.error,
        },
    }
}

/// 文档标题 token 池：`<title>` 与所有 `<h1>` 的 token 并集；只用于剔除正文首行的标题行
///（webctl 的标题是独立字段，trafilatura 的 txt 把标题放首行），不参与正文评分。
fn document_title_tokens(html: &str) -> Vec<String> {
    let document = html::parse_document(html);
    let mut tokens = Vec::new();
    if let Some(title) = html::document_title(&document) {
        tokens.extend(tokenize(&title));
    }
    if let Some(css) = html::selector("h1") {
        for element in document.select(&css) {
            tokens.extend(tokenize(&element.text().collect::<String>()));
        }
    }
    tokens
}

/// 若首行与标题 token 池互为子集（标题带站点后缀、或少量的 h1 差异），剔除该行。
fn drop_title_line(content: &str, title_tokens: &[String]) -> String {
    if title_tokens.is_empty() {
        return content.to_owned();
    }
    let Some(first) = content.lines().find(|line| !line.trim().is_empty()) else {
        return content.to_owned();
    };
    let first_tokens = tokenize(first);
    let is_title =
        contains_all(title_tokens, &first_tokens) || contains_all(&first_tokens, title_tokens);
    if !is_title {
        return content.to_owned();
    }
    let mut kept = String::new();
    let mut skipped = false;
    for line in content.lines() {
        if !skipped && !line.trim().is_empty() {
            skipped = true;
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    kept
}

/// 多重集包含：`haystack` 是否覆盖 `needles` 的每一次出现。
fn contains_all(haystack: &[String], needles: &[String]) -> bool {
    if needles.is_empty() {
        return false;
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for token in haystack {
        *counts.entry(token.as_str()).or_insert(0) += 1;
    }
    for token in needles {
        match counts.get_mut(token.as_str()) {
            Some(count) if *count > 0 => *count -= 1,
            _ => return false,
        }
    }
    true
}

fn build_report(options: &Options, manifest: &Manifest, rows: &[Value]) -> Result<Value, String> {
    let mut totals = Counts::default();
    let mut categories: Vec<(String, Counts, usize)> = Vec::new();
    for row in rows {
        let counts = Counts {
            matched: as_usize(row, "matched")?,
            predicted: as_usize(row, "predicted_paragraphs")?,
            reference: as_usize(row, "reference_paragraphs")?,
        };
        totals.add(counts);
        let category = row["category"].as_str().unwrap_or("unknown").to_owned();
        match categories.iter_mut().find(|(name, _, _)| *name == category) {
            Some((_, sums, samples)) => {
                sums.add(counts);
                *samples += 1;
            }
            None => categories.push((category, counts, 1)),
        }
    }
    let categories_json: Vec<Value> = categories
        .iter()
        .map(|(name, counts, samples)| {
            json!({
                "category": name,
                "samples": samples,
                "reference_paragraphs": counts.reference,
                "predicted_paragraphs": counts.predicted,
                "matched": counts.matched,
                "precision": round4(counts.precision()),
                "recall": round4(counts.recall()),
                "f1": round4(counts.f1()),
            })
        })
        .collect();
    let source = match &options.predictions {
        None => format!(
            "webctl parse::article ({}) (in-process)",
            options.view.label()
        ),
        Some(dir) => format!("external predictions: {}", dir.display()),
    };
    Ok(json!({
        "schema": 1,
        "metric": "paragraph-level Dice matching, one non-empty line = one paragraph, title line matching <title>/<h1> excluded; P = matched/pred, R = matched/ref; see docs/EXTRACTION-BENCHMARK.md",
        "source": source,
        "view": options.view.label(),
        "settings": {
            "threshold": options.threshold,
            "min_tokens": options.min_tokens,
            "fixtures": options.fixtures.display().to_string(),
            "fixture_samples": manifest.samples.len(),
        },
        "totals": {
            "samples": rows.len(),
            "reference_paragraphs": totals.reference,
            "predicted_paragraphs": totals.predicted,
            "matched": totals.matched,
            "precision": round4(totals.precision()),
            "recall": round4(totals.recall()),
            "f1": round4(totals.f1()),
        },
        "categories": categories_json,
        "samples": rows,
    }))
}

fn as_usize(row: &Value, key: &str) -> Result<usize, String> {
    row[key]
        .as_u64()
        .map(|value| value as usize)
        .ok_or_else(|| format!("报告行缺少整数字段 {key}"))
}

fn write_report(path: &Path, report: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("建目录 {} 失败: {error}", parent.display()))?;
    }
    let mut text =
        serde_json::to_string_pretty(report).map_err(|error| format!("序列化报告失败: {error}"))?;
    text.push('\n');
    std::fs::write(path, text).map_err(|error| format!("写报告 {} 失败: {error}", path.display()))
}

fn print_table(rows: &[Value], report: &Value) {
    println!(
        "{:<18} {:<9} {:<13} {:>6} {:>6} {:>6} {:>4} {:>4}",
        "sample", "category", "extractor", "P", "R", "F1", "ref", "pred"
    );
    for row in rows {
        println!(
            "{:<18} {:<9} {:<13} {:>6} {:>6} {:>6} {:>4} {:>4}",
            field(row, "id"),
            field(row, "category"),
            field(row, "extractor"),
            number(row, "precision"),
            number(row, "recall"),
            number(row, "f1"),
            field(row, "reference_paragraphs"),
            field(row, "predicted_paragraphs"),
        );
    }
    println!();
    println!("-- 按类别（micro：该类所有段落合计）--");
    for category in report["categories"].as_array().into_iter().flatten() {
        println!(
            "{:<12} samples={:<2} ref={:<3} pred={:<3} P={:.3} R={:.3} F1={:.3}",
            field(category, "category"),
            field(category, "samples"),
            field(category, "reference_paragraphs"),
            field(category, "predicted_paragraphs"),
            category["precision"].as_f64().unwrap_or_default(),
            category["recall"].as_f64().unwrap_or_default(),
            category["f1"].as_f64().unwrap_or_default(),
        );
    }
    let totals = &report["totals"];
    println!(
        "TOTAL        samples={:<2} ref={:<3} pred={:<3} P={:.3} R={:.3} F1={:.3}",
        field(totals, "samples"),
        field(totals, "reference_paragraphs"),
        field(totals, "predicted_paragraphs"),
        totals["precision"].as_f64().unwrap_or_default(),
        totals["recall"].as_f64().unwrap_or_default(),
        totals["f1"].as_f64().unwrap_or_default(),
    );
}

fn field(value: &Value, key: &str) -> String {
    match &value[key] {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn number(value: &Value, key: &str) -> String {
    format!("{:.3}", value[key].as_f64().unwrap_or_default())
}

/// 只打印单个样本的逐段匹配，用于人工核对 fixtures 与阈值。
fn dump_sample(options: &Options, manifest: &Manifest, id: &str) -> Result<(), String> {
    let sample = manifest
        .samples
        .iter()
        .find(|sample| sample.id == id)
        .ok_or_else(|| format!("样本不存在: {id}"))?;
    let reference_text = read_fixture(&options.fixtures, &sample.expected)?;
    let html_text = read_fixture(&options.fixtures, &sample.html)?;
    let title_tokens = document_title_tokens(&html_text);
    let (predicted_text, outcome) = match &options.predictions {
        Some(dir) => (read_prediction(dir, &sample.id)?, None),
        None => {
            let outcome = extract_with_webctl(&html_text, options.view);
            (outcome.content.clone(), Some(outcome))
        }
    };
    let predicted_text = drop_title_line(&predicted_text, &title_tokens);
    println!("sample: {}  category: {}", sample.id, sample.category);
    if let Some(outcome) = &outcome {
        println!(
            "webctl: extractor={} code={} degraded_from={:?} view={}",
            outcome.extractor,
            outcome.code,
            outcome.degraded_from,
            options.view.label()
        );
        if !outcome.error.is_empty() {
            println!("webctl error: {}", outcome.error);
        }
    }
    let reference = split_paragraphs(&reference_text);
    let predicted = split_paragraphs(&predicted_text);
    let reference_tokens: Vec<Vec<String>> = reference.iter().map(|text| tokenize(text)).collect();
    let predicted_tokens: Vec<Vec<String>> = predicted.iter().map(|text| tokenize(text)).collect();

    let keep = |tokens: &[String]| {
        if tokens.len() >= options.min_tokens {
            "keep"
        } else {
            "drop"
        }
    };
    println!("\n-- 参考段落（{}）--", reference.len());
    for (index, (text, tokens)) in reference.iter().zip(&reference_tokens).enumerate() {
        println!(
            "[{index}] {} tokens={} :: {}",
            keep(tokens),
            tokens.len(),
            snippet(text)
        );
    }
    println!("\n-- 预测段落（{}）--", predicted.len());
    for (index, (text, tokens)) in predicted.iter().zip(&predicted_tokens).enumerate() {
        println!(
            "[{index}] {} tokens={} :: {}",
            keep(tokens),
            tokens.len(),
            snippet(text)
        );
    }
    let score = score(
        &reference,
        &predicted,
        options.threshold,
        options.min_tokens,
    );
    println!(
        "\nresult: P={:.3} R={:.3} F1={:.3} (ref={} pred={} matched={})",
        score.counts.precision(),
        score.counts.recall(),
        score.counts.f1(),
        score.counts.reference,
        score.counts.predicted,
        score.counts.matched
    );
    if !score.missed.is_empty() {
        println!("\n-- 漏掉的参考段 --");
        for text in &score.missed {
            println!("- {text}");
        }
    }
    if !score.spurious.is_empty() {
        println!("\n-- 多出的预测段 --");
        for text in &score.spurious {
            println!("+ {text}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 指标实现（确定性、无依赖）
// ---------------------------------------------------------------------------

/// 一行一段：去掉首尾空白，忽略空行。
fn split_paragraphs(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// 归一化 token：拉丁/数字连续串（小写）一个 token，CJK 逐字，其余为分隔符。
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut latin = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() {
            if is_cjk(character) {
                flush_latin(&mut latin, &mut tokens);
                tokens.push(character.to_lowercase().collect());
            } else {
                latin.extend(character.to_lowercase());
            }
        } else {
            flush_latin(&mut latin, &mut tokens);
        }
    }
    flush_latin(&mut latin, &mut tokens);
    tokens
}

fn flush_latin(latin: &mut String, tokens: &mut Vec<String>) {
    if !latin.is_empty() {
        tokens.push(std::mem::take(latin));
    }
}

/// CJK 统一表意文字 + 日文假名 + 谚文：这些文字逐字成 token，避免整句一个 token。
fn is_cjk(character: char) -> bool {
    matches!(
        character as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF
    )
}

/// 多重集 Dice 系数：`2|A ∩ B| / (|A| + |B|)`，重复 token 按最小出现次数计。
fn dice(left: &[String], right: &[String]) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for token in left {
        *counts.entry(token.as_str()).or_insert(0) += 1;
    }
    let mut intersection = 0usize;
    for token in right {
        if let Some(count) = counts.get_mut(token.as_str())
            && *count > 0
        {
            *count -= 1;
            intersection += 1;
        }
    }
    2.0 * intersection as f64 / (left.len() + right.len()) as f64
}

/// 分数：先过 `min_tokens`，再做贪心一对一配对。
fn score(
    reference: &[String],
    predicted: &[String],
    threshold: f64,
    min_tokens: usize,
) -> SampleScore {
    let reference_kept: Vec<(&String, Vec<String>)> = reference
        .iter()
        .map(|text| (text, tokenize(text)))
        .filter(|(_, tokens)| tokens.len() >= min_tokens)
        .collect();
    let predicted_kept: Vec<(&String, Vec<String>)> = predicted
        .iter()
        .map(|text| (text, tokenize(text)))
        .filter(|(_, tokens)| tokens.len() >= min_tokens)
        .collect();

    let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
    for (predicted_index, (_, tokens)) in predicted_kept.iter().enumerate() {
        for (reference_index, (_, reference_tokens)) in reference_kept.iter().enumerate() {
            let similarity = dice(tokens, reference_tokens);
            if similarity >= threshold {
                candidates.push((similarity, predicted_index, reference_index));
            }
        }
    }
    candidates.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then(left.1.cmp(&right.1))
            .then(left.2.cmp(&right.2))
    });
    let mut predicted_used = vec![false; predicted_kept.len()];
    let mut reference_used = vec![false; reference_kept.len()];
    let mut matched = 0usize;
    for (_, predicted_index, reference_index) in &candidates {
        if !predicted_used[*predicted_index] && !reference_used[*reference_index] {
            predicted_used[*predicted_index] = true;
            reference_used[*reference_index] = true;
            matched += 1;
        }
    }
    let missed = reference_kept
        .iter()
        .zip(&reference_used)
        .filter(|(_, used)| !**used)
        .map(|((text, _), _)| snippet(text))
        .collect();
    let spurious = predicted_kept
        .iter()
        .zip(&predicted_used)
        .filter(|(_, used)| !**used)
        .map(|((text, _), _)| snippet(text))
        .collect();
    SampleScore {
        counts: Counts {
            matched,
            predicted: predicted_kept.len(),
            reference: reference_kept.len(),
        },
        missed,
        spurious,
    }
}

fn snippet(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= SNIPPET_CHARS {
        return collapsed;
    }
    let mut short: String = collapsed.chars().take(SNIPPET_CHARS).collect();
    short.push('…');
    short
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paras(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn tokenize_and_split() {
        assert_eq!(tokenize("Hello, WORLD's 42!").join(" "), "hello world s 42");
        assert_eq!(tokenize("中文段落 mixed").join(" "), "中 文 段 落 mixed");
        assert!(tokenize("...").is_empty());
        assert_eq!(
            split_paragraphs("first line\n\nsecond line  \n\n"),
            paras(&["first line", "second line"])
        );
    }

    #[test]
    fn dice_counts_multiplicity() {
        let (left, right) = (paras(&["a", "a", "b"]), paras(&["a", "b", "b"]));
        assert!((dice(&left, &right) - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(dice(&[], &right), 0.0);
    }

    #[test]
    fn greedy_matching_is_one_to_one() {
        let reference = paras(&["alpha beta gamma delta", "one two three four"]);
        let predicted = paras(&[
            "alpha beta gamma delta",
            "alpha beta gamma delta",
            "one two three four",
        ]);
        let result = score(&reference, &predicted, 0.6, 1);
        assert_eq!(
            (
                result.counts.matched,
                result.counts.predicted,
                result.counts.reference
            ),
            (2, 3, 2)
        );
        assert_eq!(result.spurious.len(), 1);
        assert!(result.missed.is_empty());
    }

    #[test]
    fn short_paragraphs_are_dropped_on_both_sides() {
        let reference = paras(&["too short", "a real paragraph with prose"]);
        let predicted = paras(&["menu", "a real paragraph with prose"]);
        let result = score(&reference, &predicted, 0.6, 4);
        assert_eq!((result.counts.reference, result.counts.predicted), (1, 1));
        assert_eq!(result.counts.f1(), 1.0);
    }

    #[test]
    fn title_line_is_excluded_only_for_the_title() {
        // 标题带站点后缀 / 只有 h1：都按 token 子集识别
        let content = "Client setup — Widget Docs\nbody line\n";
        assert_eq!(
            drop_title_line(content, &tokenize("Client setup")),
            "body line\n"
        );
        assert_eq!(
            drop_title_line(
                "East Harbor Seawall\nbody\n",
                &tokenize("East Harbor Seawall — Fixture")
            ),
            "body\n"
        );
        // 首行是正文、或没有标题池：不动
        let body = "We have about forty integration tests that hit a real database.\n";
        let title = tokenize("How do you handle flaky integration tests?");
        assert_eq!(drop_title_line(body, &title), body);
        assert_eq!(drop_title_line(body, &[]), body);
    }

    #[test]
    fn fixtures_manifest_is_well_formed() {
        let fixtures = Path::new(DEFAULT_FIXTURES);
        let manifest = load_manifest(fixtures).expect("fixtures manifest");
        assert!(manifest.samples.len() >= 8, "至少 8 个本地样本");
        for sample in &manifest.samples {
            let expected = read_fixture(fixtures, &sample.expected).expect("expected text");
            assert!(
                split_paragraphs(&expected).len() >= 3,
                "{} 参考段太少",
                sample.id
            );
            assert!(read_fixture(fixtures, &sample.html).expect("html").len() > 200);
        }
    }
}
