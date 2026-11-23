//! sitemap.xml / sitemap index 的**离线**解析（议题 #196 第 3 步）。
//!
//! 只做两件事：按根元素区分 urlset / sitemapindex，并把 <loc> 取出来。
//! **不联网、不递归**——递归取回由 channels::fetch_sitemap 负责，并带深度/文档/条目上限。
//!
//! 诚实性：认不出的文档返回 kind=Unknown + 空列表（**不是错误**）；
//! "取到了但没有 URL"与"取不到"由上层用状态区分，不在本模块猜。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::embedded::unescape_entities;

/// 单个 sitemap 文档最多解析多少条 <loc>。
pub const MAX_SITEMAP_ENTRIES: usize = 50;

/// 一次发现最多取回/展开多少个 sitemap 文档（robots 声明的入口也受此限）。
pub const MAX_SITEMAP_CHILDREN: usize = 5;

/// 递归层数上限：0 = 入口文档；1 = index 的子 sitemap；更深的 index 不再展开。
pub const MAX_SITEMAP_DEPTH: usize = 2;

/// sitemap 文档类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SitemapKind {
    /// <urlset>：locs 是页面 URL。
    UrlSet,
    /// <sitemapindex>：locs 是子 sitemap 地址。
    Index,
    /// 既不是 urlset 也不是 sitemapindex。
    Unknown,
}

/// 一个 sitemap 文档的解析结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SitemapDoc {
    /// 文档类型。
    pub kind: SitemapKind,
    /// <loc> 列表（去重、保序、只收 http(s) 绝对地址）。
    pub locs: Vec<String>,
    /// 是否因 MAX_SITEMAP_ENTRIES 被截断。
    pub truncated: bool,
    /// 可读说明（认不出的结构写在这里）。
    pub note: String,
}

/// 解析一个 sitemap 文档（纯离线，不跟随 index）。
pub fn parse_sitemap(xml: &str) -> SitemapDoc {
    let lower = xml.to_ascii_lowercase();
    let index_at = lower.find("<sitemapindex");
    let urlset_at = lower.find("<urlset");
    let kind = match (index_at, urlset_at) {
        (Some(index), Some(urlset)) if index < urlset => SitemapKind::Index,
        (Some(_), Some(_)) => SitemapKind::UrlSet,
        (Some(_), None) => SitemapKind::Index,
        (None, Some(_)) => SitemapKind::UrlSet,
        (None, None) => SitemapKind::Unknown,
    };
    if kind == SitemapKind::Unknown {
        return SitemapDoc {
            kind,
            locs: Vec::new(),
            truncated: false,
            note: "既不是 <urlset> 也不是 <sitemapindex>".to_owned(),
        };
    }
    let (locs, truncated) = collect_locs(&lower, xml, MAX_SITEMAP_ENTRIES);
    let label = if kind == SitemapKind::Index {
        "sitemapindex"
    } else {
        "urlset"
    };
    let note = format!("{label}，{} 个 <loc>", locs.len());
    SitemapDoc {
        kind,
        locs,
        truncated,
        note,
    }
}

/// 取 <loc>…</loc> 的文本：实体还原、去空白、只收 http(s) 绝对地址、按首次出现去重。
fn collect_locs(lower: &str, xml: &str, limit: usize) -> (Vec<String>, bool) {
    let mut out: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut cursor = 0usize;
    let mut truncated = false;
    while let Some(rel) = lower[cursor..].find("<loc") {
        let at = cursor + rel;
        let after = at + "<loc".len();
        let boundary = lower[after..].chars().next();
        if !matches!(boundary, Some(ch) if ch.is_whitespace() || ch == '>') {
            cursor = after;
            continue;
        }
        let Some(gt_rel) = lower[after..].find('>') else {
            break;
        };
        let gt = after + gt_rel;
        let Some(end_rel) = lower[gt..].find("</loc>") else {
            break;
        };
        let end = gt + end_rel;
        let value = unescape_entities(&xml[gt + 1..end]);
        let value = value.trim();
        if is_http_url(value) && seen.insert(value.to_owned()) {
            if out.len() >= limit {
                truncated = true;
                break;
            }
            out.push(value.to_owned());
        }
        cursor = end + "</loc>".len();
    }
    (out, truncated)
}

fn is_http_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && value.len() > "https://".len()
}
#[cfg(test)]
mod tests {
    use super::*;

    const URLSET: &str = r#"<?xml version="1.0"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
      <url><loc>https://example.test/a</loc></url>
      <url><loc> https://example.test/b?x=1&amp;y=2 </loc></url>
      <url><loc>https://example.test/a</loc></url>
      <url><loc>/relative</loc></url>
    </urlset>"#;

    /// urlset：保序、去重、实体还原，相对地址不收。
    #[test]
    fn urlset_locs_are_ordered_deduped_and_decoded() {
        let doc = parse_sitemap(URLSET);
        assert_eq!(doc.kind, SitemapKind::UrlSet);
        assert_eq!(
            doc.locs,
            vec![
                "https://example.test/a".to_owned(),
                "https://example.test/b?x=1&y=2".to_owned()
            ]
        );
        assert!(!doc.truncated);
    }

    /// 结构识别与边界：index 识别、认不出不是错误、超限显式截断、`<location>` 不算 `<loc>`。
    #[test]
    fn structure_limits_and_tag_boundaries_are_explicit() {
        let index = parse_sitemap(
            r#"<sitemapindex><sitemap><loc>https://example.test/s1.xml</loc></sitemap></sitemapindex>"#,
        );
        assert_eq!(index.kind, SitemapKind::Index);
        assert_eq!(index.locs, vec!["https://example.test/s1.xml".to_owned()]);

        let unknown = parse_sitemap("<html>not a sitemap</html>");
        assert_eq!(unknown.kind, SitemapKind::Unknown);
        assert!(unknown.locs.is_empty() && !unknown.note.is_empty());

        let mut many = String::from("<urlset>");
        for index in 0..(MAX_SITEMAP_ENTRIES + 5) {
            many.push_str(&format!(
                "<url><loc>https://example.test/{index}</loc></url>"
            ));
        }
        many.push_str("</urlset>");
        let doc = parse_sitemap(&many);
        assert_eq!(doc.locs.len(), MAX_SITEMAP_ENTRIES);
        assert!(doc.truncated, "超过上限必须显式标注截断");

        let location =
            parse_sitemap("<urlset><location>https://example.test/x</location></urlset>");
        assert!(location.locs.is_empty(), "location 标签不该被当成 loc");
    }
}
