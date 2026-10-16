//! 正文提取（PARSE-01/02/03/10/13/16）。
//!
//! - 主路径：dom_smoothie（Readability 算法 + TextMode::Markdown）；
//! - 纯文本视图：rust-html2text 渲染主路径正文 HTML；
//! - 回退：显式剥离噪声容器后整页渲染，并作为**显式降级**上报
//!   （code=DEGRADED, meta.degraded_from="dom_smoothie"）。
//!
//! 回退**不是静默**的：调用方从 code / degraded_from / extractor 三处都能看到
//! 主路径没有生效。整页在剥离噪声后仍无文本时返回 code=PARSE（不是空成功）。

use std::time::Instant;

use dom_smoothie::{Config, Readability, TextMode};
use serde::{Deserialize, Serialize};
use url::Url;
use webctl_core::{Envelope, ErrorCode, Trust};

use super::{elapsed_ms, html};

/// 正文提取结果：同一份正文的纯文本与 markdown 两种视图。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Article {
    /// 文档标题（缺失时为空字符串）。
    pub title: String,
    /// 作者署名（Readability 能识别时）。
    pub byline: Option<String>,
    /// 文档语言（html lang 或 Readability 判定）。
    pub lang: Option<String>,
    /// 纯文本视图（rust-html2text）。
    pub text: String,
    /// markdown 视图（dom_smoothie TextMode::Markdown / 回退渲染器）。
    pub markdown: String,
    /// 实际生效的提取器："dom_smoothie" 或 "fallback"。
    pub extractor: String,
}

/// 正文提取主入口。
///
/// 语义：
/// - 成功且主路径生效 → ok=true, code=OK；
/// - 主路径失败但回退可用 → ok=true, code=DEGRADED, degraded_from="dom_smoothie"；
/// - 空输入或剥离噪声后无任何文本 → ok=false, code=PARSE。
pub fn extract_article(html_text: &str, base: Option<&str>) -> Envelope<Article> {
    let started = Instant::now();
    if html_text.trim().is_empty() {
        return Envelope::fail(
            ErrorCode::Parse,
            "empty input: nothing to parse",
            elapsed_ms(started),
        );
    }
    // 极深嵌套先走回退：dom_smoothie 在这种输入上会退化到分钟级。
    let document = html::parse_document(html_text);
    if html::max_element_depth(&document) > html::MAX_READABILITY_DEPTH {
        return fallback(html_text, started);
    }
    let absolute_base = base.filter(|value| Url::parse(value).is_ok());
    let mut config = Config::default();
    config.text_mode = TextMode::Markdown;
    match Readability::new(html_text, absolute_base, Some(config)) {
        Ok(mut readability) => match readability.parse() {
            Ok(article) => {
                let markdown = article.text_content.trim().to_string();
                let text = html::render_plain_text(&article.content);
                if markdown.is_empty() || text.trim().is_empty() {
                    return fallback(html_text, started);
                }
                Envelope::ok(
                    Article {
                        title: article.title.trim().to_string(),
                        byline: article.byline,
                        lang: article.lang,
                        text,
                        markdown,
                        extractor: "dom_smoothie".to_owned(),
                    },
                    Trust::UntrustedPageContent,
                    elapsed_ms(started),
                )
            }
            Err(_) => fallback(html_text, started),
        },
        Err(_) => fallback(html_text, started),
    }
}

/// 直接走整页回退路径（供调用方显式绕过 Readability，或做对照）。
///
/// base 参与相对链接解析的语义由调用方负责；本函数只做噪声剥离与渲染，
/// 因此这里接受并忽略 base。
pub fn extract_article_fallback(html_text: &str, _base: Option<&str>) -> Envelope<Article> {
    let started = Instant::now();
    fallback(html_text, started)
}

fn fallback(html_text: &str, started: Instant) -> Envelope<Article> {
    let document = html::parse_document(html_text);
    let cleaned = html::cleaned_body_html(&document);
    let text = html::render_plain_text(&cleaned);
    let markdown = html::render_markdown(&cleaned);
    if text.trim().is_empty() && markdown.trim().is_empty() {
        return Envelope::fail(
            ErrorCode::Parse,
            "article extraction failed: no readable text after noise removal",
            elapsed_ms(started),
        );
    }
    let title = html::document_title(&document).unwrap_or_default();
    Envelope::degraded(
        Article {
            title,
            byline: None,
            lang: None,
            text,
            markdown,
            extractor: "fallback".to_owned(),
        },
        "dom_smoothie",
        ErrorCode::Degraded,
        elapsed_ms(started),
    )
}
