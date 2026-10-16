//! JSON / XML 结构化解析与内容类型路由（PARSE-11/15）。
//!
//! - JSON 用 serde_json 全量解析；XML 用 roxmltree 建树后转成等价的
//!   JSON 对象树（tag / attrs / children，文本节点为字符串）。
//! - 两者都在 raw 字段保留原始文本，可回溯到源输入。
//! - 解析失败 → ok=false + code=PARSE；空输入同样是 PARSE，不当空结果。
//! - parse_body 按 PR 02 的 ContentKind 路由：html/json/xml 各走对应处理器；
//!   pdf/binary 不硬当文本，返回 code=UNSUPPORTED 的显式降级。

use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use webctl_core::{Envelope, ErrorCode, Trust};

use super::{article, elapsed_ms};
use crate::content_type::ContentKind;
use crate::transport::Body;

/// 结构化解析结果（JSON 或 XML）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Structured {
    /// "json" 或 "xml"。
    pub format: String,
    /// XML 根元素名；JSON 为 None。
    pub root: Option<String>,
    /// 原始文本字节数（UTF-8 长度）。
    pub bytes: usize,
    /// 原始文本，可回溯。
    pub raw: String,
    /// 结构化树。
    pub value: Value,
}

/// 按内容类型解析后的联合结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "snake_case")]
pub enum Parsed {
    /// HTML 正文。
    Html(article::Article),
    /// JSON。
    Json(Structured),
    /// XML。
    Xml(Structured),
    /// 非文本内容：显式不支持，交给下载/文档处理器。
    Unsupported {
        /// 内容分支名。
        kind: String,
        /// 可读原因。
        reason: String,
    },
}

/// 解析 JSON 文本。
pub fn parse_json(text: &str) -> Envelope<Structured> {
    let started = Instant::now();
    if text.trim().is_empty() {
        return Envelope::fail(ErrorCode::Parse, "empty JSON input", elapsed_ms(started));
    }
    match serde_json::from_str::<Value>(text) {
        Ok(value) => Envelope::ok(
            Structured {
                format: "json".to_owned(),
                root: None,
                bytes: text.len(),
                raw: text.to_owned(),
                value,
            },
            Trust::UntrustedPageContent,
            elapsed_ms(started),
        ),
        Err(error) => Envelope::fail(
            ErrorCode::Parse,
            format!("invalid JSON: {error}"),
            elapsed_ms(started),
        ),
    }
}

/// 解析 XML 文本为结构化树。
pub fn parse_xml(text: &str) -> Envelope<Structured> {
    let started = Instant::now();
    if text.trim().is_empty() {
        return Envelope::fail(ErrorCode::Parse, "empty XML input", elapsed_ms(started));
    }
    match roxmltree::Document::parse(text) {
        Ok(document) => {
            let root = document.root_element();
            let root_name = root.tag_name().name().to_owned();
            Envelope::ok(
                Structured {
                    format: "xml".to_owned(),
                    root: Some(root_name),
                    bytes: text.len(),
                    raw: text.to_owned(),
                    value: xml_node_to_value(root),
                },
                Trust::UntrustedPageContent,
                elapsed_ms(started),
            )
        }
        Err(error) => Envelope::fail(
            ErrorCode::Parse,
            format!("invalid XML: {error}"),
            elapsed_ms(started),
        ),
    }
}

fn xml_node_to_value(node: roxmltree::Node<'_, '_>) -> Value {
    let mut object = Map::new();
    object.insert(
        "tag".to_owned(),
        Value::String(node.tag_name().name().to_owned()),
    );
    if node.attributes().len() > 0 {
        let mut attrs = Map::new();
        for attribute in node.attributes() {
            attrs.insert(
                attribute.name().to_owned(),
                Value::String(attribute.value().to_owned()),
            );
        }
        object.insert("attrs".to_owned(), Value::Object(attrs));
    }
    let mut children: Vec<Value> = Vec::new();
    for child in node.children() {
        if child.is_element() {
            children.push(xml_node_to_value(child));
        } else if child.is_text()
            && let Some(text) = child.text()
            && !text.trim().is_empty()
        {
            children.push(Value::String(text.to_owned()));
        }
    }
    if !children.is_empty() {
        object.insert("children".to_owned(), Value::Array(children));
    }
    Value::Object(object)
}

/// 按内容类型路由解析。
///
/// - html → 正文提取（可能 DEGRADED）；
/// - json / xml → 对应结构化解析（PARSE 或 OK）；
/// - pdf / binary → code=UNSUPPORTED 的显式降级，data 说明为何不解析。
pub fn parse_body(body: &Body, base: Option<&str>) -> Envelope<Parsed> {
    let started = Instant::now();
    match body.kind {
        ContentKind::Html => match body.text.as_deref() {
            Some(text) => rewrap(article::extract_article(text, base), Parsed::Html),
            None => missing_text(started),
        },
        ContentKind::Json => match body.text.as_deref() {
            Some(text) => rewrap(parse_json(text), Parsed::Json),
            None => missing_text(started),
        },
        ContentKind::Xml => match body.text.as_deref() {
            Some(text) => rewrap(parse_xml(text), Parsed::Xml),
            None => missing_text(started),
        },
        ContentKind::Pdf | ContentKind::Binary => Envelope::degraded(
            Parsed::Unsupported {
                kind: body.kind.as_str().to_owned(),
                reason: format!(
                    "{} content is not text; route it to the document/download handler",
                    body.kind.as_str()
                ),
            },
            format!("content-type:{}", body.kind.as_str()),
            ErrorCode::Unsupported,
            elapsed_ms(started),
        ),
    }
}

fn missing_text(started: Instant) -> Envelope<Parsed> {
    Envelope::fail(
        ErrorCode::Internal,
        "content-type routing expected decoded text but body.text is None",
        elapsed_ms(started),
    )
}

fn rewrap<T>(envelope: Envelope<T>, wrap: impl FnOnce(T) -> Parsed) -> Envelope<Parsed> {
    let Envelope {
        ok,
        data,
        error,
        code,
        meta,
    } = envelope;
    Envelope {
        ok,
        data: data.map(wrap),
        error,
        code,
        meta,
    }
}
