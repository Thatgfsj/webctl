//! 解析/抽取性能探针（议题 #83 / TESTING §1 的 L4 通道）。
//!
//! 为什么需要它：PR #80 把 PARSE-14 / EXTRACT-14 / EXTRACT-17 的墙钟期望放宽到 30s
//! 挂死守卫后，`[~5s, 30s)` 这一档在单测里**没有门禁**（单测不该赌机器负载）。
//! 本探针用**确定性输入**测 p50，把「量级级退化」交给 scripts/perf-baseline.sh 判定。
//!
//! 关键：默认 `ExtractOptions` 是**面向用户输出**的（max_results=50、max_elapsed_ms=5000），
//! 用它测不出扫描成本——而 PR 06 的缺陷恰恰是「零命中大页面绕过预算」。所以这里显式放开
//! 结果上限与墙钟预算，专门测**全量扫描**。
//!
//! 用法：
//!   cargo run --release --example perf_probe -- --json
//!   PERF_RUNS=11 cargo run --release --example perf_probe --

use std::time::Instant;

use webctl_web::extract::{ExtractOptions, select_matches};
use webctl_web::parse;

/// 生成约 100 KiB 的嵌套 HTML（对应 PARSE-14 的量级）。
fn html_100kb() -> String {
    let mut s = String::from("<html><body>");
    let mut i = 0usize;
    while s.len() < 100 * 1024 {
        s.push_str(&format!(
            "<div class=\"row r{i}\"><h2>Title {i}</h2><p>Lorem ipsum dolor sit amet {i}</p><a href=\"/x/{i}\">next</a></div>"
        ));
        i += 1;
    }
    s.push_str("</body></html>");
    s
}

/// 生成约 1 MiB 的列表页（很多链接/表格/表单，对应 EXTRACT-14 的量级）。
fn html_1mb() -> String {
    let mut s = String::from("<html><body><table><tbody>");
    let mut i = 0usize;
    while s.len() < 1024 * 1024 {
        s.push_str(&format!(
            "<tr><td><a href=\"/i/{i}\">item {i}</a></td><td>{i}</td></tr>"
        ));
        i += 1;
    }
    s.push_str("</tbody></table>");
    for j in 0..50 {
        s.push_str(&format!(
            "<form action=\"/f/{j}\"><input name=\"q\" value=\"{j}\"><input type=\"hidden\" name=\"csrf\" value=\"t{j}\"></form>"
        ));
    }
    s.push_str("</body></html>");
    s
}

/// 深嵌套 HTML（对应 EXTRACT-17 的深链截断路径）。
fn html_deep() -> String {
    let mut s = String::from("<html><body>");
    for _ in 0..2_000 {
        s.push_str("<div>");
    }
    s.push_str("<a href=\"/deep\">deep</a>");
    for _ in 0..2_000 {
        s.push_str("</div>");
    }
    s.push_str("</body></html>");
    s
}

/// 放开输出上限与墙钟预算：测的是**扫描**，不是「用户可见输出的构造」。
fn scan_options() -> ExtractOptions {
    ExtractOptions {
        max_results: 100_000,
        char_budget: 64 * 1024 * 1024,
        max_html_chars: 128,
        max_elapsed_ms: 120_000,
        max_scan_matches: 2_000_000,
        ..ExtractOptions::default()
    }
}

fn p50(mut f: impl FnMut(), runs: usize) -> f64 {
    let mut v = Vec::with_capacity(runs);
    for _ in 0..runs {
        let t = Instant::now();
        f();
        v.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

fn main() {
    let json = std::env::args().any(|a| a == "--json");
    let runs: usize = std::env::var("PERF_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(7);

    let h100 = html_100kb();
    let h1m = html_1mb();
    let deep = html_deep();
    let opts = scan_options();

    let parse_100kb = p50(
        || {
            let _ = parse::select(&h100, "div.row");
        },
        runs,
    );
    let extract_1mb_hit = p50(
        || {
            let _ = select_matches(&h1m, "a", &opts);
        },
        runs,
    );
    // 零命中 → 必须把整页扫完（PR 06 的缺陷就在这条路径上：曾经绕过 max_elapsed_ms）。
    let extract_1mb_nohit = p50(
        || {
            let _ = select_matches(&h1m, "article.nonexistent", &opts);
        },
        runs,
    );
    let extract_deep = p50(
        || {
            let _ = select_matches(&deep, "a", &opts);
        },
        runs,
    );

    if json {
        println!(
            "{{\"parse_100kb_ms_p50\":{parse_100kb:.3},\"extract_1mb_hit_ms_p50\":{extract_1mb_hit:.3},\"extract_1mb_nohit_ms_p50\":{extract_1mb_nohit:.3},\"extract_deep_ms_p50\":{extract_deep:.3},\"runs\":{runs}}}"
        );
    } else {
        println!("parse   100KiB div.row        p50 = {parse_100kb:.3} ms  (runs={runs})");
        println!("extract 1MiB   a (有命中)     p50 = {extract_1mb_hit:.3} ms  (runs={runs})");
        println!("extract 1MiB   零命中全量扫描 p50 = {extract_1mb_nohit:.3} ms  (runs={runs})");
        println!("extract deep(2k) a            p50 = {extract_deep:.3} ms  (runs={runs})");
    }
}
