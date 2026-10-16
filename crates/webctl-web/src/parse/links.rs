//! 链接抽取（PARSE-05/06）。
//!
//! - 命中 a[href] 与 area[href]，按文档序返回；
//! - 相对 href 按文档 URL 与第一个 base[href] 绝对化（base 优先于文档 URL）；
//! - rel 按空白拆分、小写、保序去重；
//! - 无法绝对化（缺 base 或 href 非法）时 href 保留原值并标 absolute=false，
//!   不丢链接、不伪造绝对地址。

use std::time::Instant;

use serde::{Deserialize, Serialize};
use webctl_core::{Envelope, ErrorCode, Trust};

use super::{elapsed_ms, html};

/// 一条链接。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// 文档序序号（0 起）。
    pub index: usize,
    /// 原始 href 属性值。
    pub raw_href: String,
    /// 绝对化后的 href；无法绝对化时等于 raw_href。
    pub href: String,
    /// href 是否成功绝对化。
    pub absolute: bool,
    /// 链接文本（折叠空白）。
    pub text: String,
    /// rel 属性：小写、保序、去重。
    pub rel: Vec<String>,
    /// title 属性。
    pub title: Option<String>,
    /// target 属性。
    pub target: Option<String>,
}

/// 抽取全部链接。零命中是合法空结果：ok=true 且 data 为空数组。
pub fn extract_links(html_text: &str, base: Option<&str>) -> Envelope<Vec<Link>> {
    let started = Instant::now();
    let css = match html::compile_selector("a[href], area[href]") {
        Ok(css) => css,
        Err(message) => return Envelope::fail(ErrorCode::Parse, message, elapsed_ms(started)),
    };
    let document = html::parse_document(html_text);
    let resolved_base = html::resolve_base(&document, base);
    let links = document
        .select(&css)
        .enumerate()
        .map(|(index, element)| {
            let raw_href = element.attr("href").unwrap_or_default().to_owned();
            let resolved = resolved_base
                .as_ref()
                .and_then(|base_url| base_url.join(&raw_href).ok());
            let (href, absolute) = match resolved {
                Some(url) => (url.to_string(), true),
                None => (raw_href.clone(), false),
            };
            Link {
                index,
                raw_href,
                href,
                absolute,
                text: html::collapse_ws(&element.text().collect::<String>()),
                rel: parse_rel(element.attr("rel")),
                title: element.attr("title").map(str::to_owned),
                target: element.attr("target").map(str::to_owned),
            }
        })
        .collect();
    Envelope::ok(links, Trust::UntrustedPageContent, elapsed_ms(started))
}

fn parse_rel(value: Option<&str>) -> Vec<String> {
    let mut rel: Vec<String> = Vec::new();
    for token in value.unwrap_or_default().split_whitespace() {
        let token = token.to_ascii_lowercase();
        if !rel.contains(&token) {
            rel.push(token);
        }
    }
    rel
}
