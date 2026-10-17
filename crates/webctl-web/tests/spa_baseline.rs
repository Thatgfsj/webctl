//! #119 SPA 样本集与"有效信息获取率"**可执行基线**。
//!
//! 目的：没有基线就无法证明 0.1.2 真的改善了（防"感觉变好了"）。
//! 这里把 5 类 SPA 形态的**当前实际结果**钉成断言——引擎接线后本文件必须显式更新，
//! 于是"改善"与"回归"都会变成可见的 diff，而不是口头结论。
//!
//! 分级（`obtained`）：`content`（正文）> `embedded`（内嵌数据）> `metadata`（元数据）> `none`。
//! 指标不是 HTTP 200 —— 拿到 200 但什么都没抽到，记 `none`。

use webctl_web::degrade::{Assessment, assess};
use webctl_web::parse::{EmbeddedOptions, extract_article, extract_embedded};

const NEXTJS: &str = include_str!("fixtures/spa/nextjs_ld_json.html");
const NUXT: &str = include_str!("fixtures/spa/nuxt_inline_state.html");
const CSR: &str = include_str!("fixtures/spa/csr_empty_shell.html");
const JSONLD_ARTICLE: &str = include_str!("fixtures/spa/jsonld_article.html");
const RSS_BLOG: &str = include_str!("fixtures/spa/rss_blog.html");

/// 单个样本的实测结果。
#[derive(Debug, PartialEq, Eq)]
struct Probe {
    /// 正文文本长度（trim 后）。
    content_chars: usize,
    /// 内嵌结构化载荷条数。
    embedded_entries: usize,
    /// 内嵌载荷来源（如 ld+json / script-json / inline-state）。
    embedded_sources: Vec<String>,
    /// 是否被判为"依赖 JS"。
    needs_js: bool,
    /// HTML 头部是否有 RSS 发现链接。
    has_rss_link: bool,
}

impl Probe {
    /// 有效信息分级（`content` > `embedded` > `metadata` > `none`）。
    fn obtained(&self) -> &'static str {
        if self.content_chars >= 40 {
            "content"
        } else if self.embedded_entries > 0 {
            "embedded"
        } else if self.has_rss_link {
            "metadata"
        } else {
            "none"
        }
    }
}

fn probe(html: &str) -> Probe {
    let article = extract_article(html, None);
    let content_chars = article
        .data
        .as_ref()
        .map(|article| article.text.trim().chars().count())
        .unwrap_or(0);
    let embedded = extract_embedded(html, &EmbeddedOptions::default());
    let (embedded_entries, embedded_sources) = match embedded.data.as_ref() {
        Some(report) => (
            report.entries.len(),
            report
                .entries
                .iter()
                .map(|entry| entry.source.clone())
                .collect(),
        ),
        None => (0, Vec::new()),
    };
    let needs_js = matches!(assess(html), Assessment::NeedsJs { .. });
    let has_rss_link = html.contains("application/rss+xml");
    Probe {
        content_chars,
        embedded_entries,
        embedded_sources,
        needs_js,
        has_rss_link,
    }
}

/// #119 基线：5 类 SPA 形态在**当前版本**下的实际取值。
///
/// 更新本表 = 声明"行为有意改变了"，请同步 `docs/SPA-BASELINE.md` 并说明原因。
#[test]
fn spa_baseline_matches_recorded_numbers() {
    let cases: [(&str, &str, Probe); 5] = [
        (
            "nextjs（JSON-LD + __NEXT_DATA__，HTML 空壳）",
            NEXTJS,
            Probe {
                content_chars: 0,
                embedded_entries: 2,
                embedded_sources: vec!["ld+json".into(), "script-json".into()],
                needs_js: true,
                has_rss_link: false,
            },
        ),
        (
            "nuxt（window.__NUXT__ 内联状态，HTML 空壳）",
            NUXT,
            Probe {
                content_chars: 0,
                embedded_entries: 1,
                embedded_sources: vec!["inline-state".into()],
                needs_js: true,
                has_rss_link: false,
            },
        ),
        (
            "纯 CSR 空壳（无任何数据）",
            CSR,
            Probe {
                content_chars: 0,
                embedded_entries: 0,
                embedded_sources: vec![],
                needs_js: true,
                has_rss_link: false,
            },
        ),
        (
            "静态文章 + JSON-LD（正文在 HTML 里）",
            JSONLD_ARTICLE,
            Probe {
                content_chars: 128,
                embedded_entries: 1,
                embedded_sources: vec!["ld+json".into()],
                needs_js: false,
                has_rss_link: false,
            },
        ),
        (
            "静态博客列表 + RSS 发现链接",
            RSS_BLOG,
            Probe {
                content_chars: 119,
                embedded_entries: 0,
                embedded_sources: vec![],
                needs_js: false,
                has_rss_link: true,
            },
        ),
    ];

    let mut rows = Vec::new();
    let mut mismatches = Vec::new();
    for (name, html, expected) in cases {
        let actual = probe(html);
        rows.push(format!(
            "| {name} | {} | {} | {} | {} | {} | {} |",
            actual.content_chars,
            actual.embedded_entries,
            if actual.embedded_sources.is_empty() {
                "-".to_owned()
            } else {
                actual.embedded_sources.join("+")
            },
            if actual.needs_js { "是" } else { "否" },
            if actual.has_rss_link { "有" } else { "无" },
            actual.obtained()
        ));
        if actual != expected {
            mismatches.push(format!("  {name}: 实测={actual:?} 基线={expected:?}"));
        }
    }
    println!("| 样本 | 正文字符 | 内嵌条数 | 内嵌来源 | 判为依赖 JS | RSS 链接 | obtained |");
    println!("|---|---|---|---|---|---|---|");
    for row in &rows {
        println!("{row}");
    }
    assert!(
        mismatches.is_empty(),
        "有 {} 个样本与基线不一致（改行为就要显式改这张表，并同步 docs/SPA-BASELINE.md）：\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

/// #119：分级函数本身要能区分"拿到 200 但什么都没有"与"拿到了"。
#[test]
fn obtained_distinguishes_empty_from_useful() {
    let empty = Probe {
        content_chars: 0,
        embedded_entries: 0,
        embedded_sources: vec![],
        needs_js: true,
        has_rss_link: false,
    };
    assert_eq!(empty.obtained(), "none", "200 但抽不到东西必须记 none");
    let embedded_only = Probe {
        embedded_entries: 1,
        ..empty
    };
    assert_eq!(embedded_only.obtained(), "embedded");
}
