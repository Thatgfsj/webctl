//! JSON / XML 结构化解析与内容类型路由（PARSE-11/15）。
//!
//! - JSON 用 serde_json 全量解析；XML 用 roxmltree 建树后转成等价的
//!   JSON 对象树（tag / attrs / children，文本节点为字符串）。
//! - 两者都在 raw 字段保留原始文本，可回溯到源输入。
//! - 解析失败 → ok=false + code=PARSE；空输入同样是 PARSE，不当空结果。
//! - roxmltree 0.21 的 tokenizer 是递归下降（parse_element↔parse_content），
//!   ParsingOptions 只有 nodes_limit、没有深度选项。因此进入 roxmltree 之前
//!   先用**线性字节扫描**做深度预检：超过 MAX_XML_DEPTH 直接 PARSE，不递归、
//!   不建树。扫描要正确处理注释 / CDATA / 处理指令 / DOCTYPE 与属性引号。
//! - 节点转 Value 用显式栈的迭代实现，消除第二处递归。
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

/// 允许进入 roxmltree 的最大元素嵌套深度。与 HTML 路径的
/// `html::MAX_READABILITY_DEPTH` 取同值 128，保持两条路径语义一致。
///
/// 取值依据：正常文档（RSS / Atom / SOAP / sitemap）嵌套在十几层以内；
/// 128 留足余量；而 roxmltree 的递归下降在 2MiB 线程栈上约 150 层即溢出，
/// 128 与实测崩溃点之间留有安全间隔。
pub const MAX_XML_DEPTH: usize = 128;

/// 解析 XML 文本为结构化树。
///
/// 在调用 roxmltree **之前**先做线性深度预检：超深输入直接返回 code=PARSE，
/// 绝不进入递归下降 tokenizer（否则约 150 层即 SIGABRT，信封与 catch_unwind
/// 均无法拦截）。
pub fn parse_xml(text: &str) -> Envelope<Structured> {
    let started = Instant::now();
    if text.trim().is_empty() {
        return Envelope::fail(ErrorCode::Parse, "empty XML input", elapsed_ms(started));
    }
    let depth = max_xml_depth(text);
    if depth > MAX_XML_DEPTH {
        return Envelope::fail(
            ErrorCode::Parse,
            format!(
                "XML nesting depth {depth} exceeds limit {MAX_XML_DEPTH}; refusing to parse untrusted deeply nested XML"
            ),
            elapsed_ms(started),
        );
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

/// 线性扫描 XML 文本，返回最大元素嵌套深度（含根元素）。
///
/// 本函数只按字节前进、零递归、零分配额外缓冲区，因此对不受信输入是安全的。
/// 它是**保守**的：只统计明确的开始 / 结束标签，正确处理
/// - 注释 `<!-- ... -->`、
/// - CDATA `<![CDATA[ ... ]]>`、
/// - 处理指令 `<? ... ?>`、
/// - DOCTYPE / 其它声明（含内部子集 `[ ... ]` 与其中的注释）、
/// - 属性值引号内的 `<` 与 `>`（不会被误判为标签边界）。
///
/// 无法识别的结构会让扫描直接冲到文本末尾（返回当前深度），绝不会陷入
/// 递归或死循环；宁可保守拒绝，也不把不受信输入交给递归解析器。
pub fn max_xml_depth(text: &str) -> usize {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut depth = 0usize;
    let mut max_depth = 0usize;
    while i < len {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        match bytes.get(i + 1).copied() {
            // 注释 / CDATA / 声明。
            Some(b'!') => {
                if bytes[i..].starts_with(b"<!--") {
                    i = match find_bytes(bytes, b"-->", i + 4) {
                        Some(end) => end + 3,
                        None => len,
                    };
                } else if bytes[i..].starts_with(b"<![CDATA[") {
                    i = match find_bytes(bytes, b"]]>", i + 9) {
                        Some(end) => end + 3,
                        None => len,
                    };
                } else {
                    i = skip_declaration(bytes, i + 2);
                }
            }
            // 处理指令。
            Some(b'?') => {
                i = match find_bytes(bytes, b"?>", i + 2) {
                    Some(end) => end + 2,
                    None => len,
                };
            }
            // 结束标签：深度减一。
            Some(b'/') => {
                i = match find_bytes(bytes, b">", i + 2) {
                    Some(end) => end + 1,
                    None => len,
                };
                depth = depth.saturating_sub(1);
            }
            // 开始标签（或自闭合标签）。
            Some(_) => match scan_start_tag(bytes, i) {
                Some((end, self_closing)) => {
                    if !self_closing {
                        depth += 1;
                        if depth > max_depth {
                            max_depth = depth;
                        }
                    }
                    i = end;
                }
                None => i = len,
            },
            None => i = len,
        }
    }
    max_depth
}

/// 扫描一个开始标签：返回 `>` 之后的下标，以及是否自闭合（`/>`）。
fn scan_start_tag(bytes: &[u8], start: usize) -> Option<(usize, bool)> {
    let len = bytes.len();
    let mut i = start + 1;
    let mut quote: Option<u8> = None;
    let mut previous_significant = 0u8;
    while i < len {
        let byte = bytes[i];
        match quote {
            Some(mark) => {
                if byte == mark {
                    quote = None;
                }
            }
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'>' => return Some((i + 1, previous_significant == b'/')),
                b' ' | b'\t' | b'\r' | b'\n' => {}
                _ => previous_significant = byte,
            },
        }
        i += 1;
    }
    None
}

/// 跳过 `<!DOCTYPE ...>` 之类声明；尊重引号与 DTD 内部子集 `[ ... ]`。
fn skip_declaration(bytes: &[u8], start: usize) -> usize {
    let len = bytes.len();
    let mut i = start;
    let mut quote: Option<u8> = None;
    let mut brackets = 0usize;
    while i < len {
        if bytes[i..].starts_with(b"<!--") {
            i = match find_bytes(bytes, b"-->", i + 4) {
                Some(end) => end + 3,
                None => return len,
            };
            continue;
        }
        let byte = bytes[i];
        match quote {
            Some(mark) => {
                if byte == mark {
                    quote = None;
                }
                i += 1;
            }
            None => match byte {
                b'"' | b'\'' => {
                    quote = Some(byte);
                    i += 1;
                }
                b'[' => {
                    brackets += 1;
                    i += 1;
                }
                b']' => {
                    brackets = brackets.saturating_sub(1);
                    i += 1;
                }
                b'>' if brackets == 0 => return i + 1,
                _ => i += 1,
            },
        }
    }
    len
}

/// 在字节串中从 `from` 起查找子串，返回其起始下标。
fn find_bytes(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= haystack.len() || needle.is_empty() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset)
}

/// 迭代建值的一帧：保存已组装的 tag/attrs、已完成的子节点，以及下一个待处理子节点。
struct XmlFrame<'a, 'input> {
    object: Map<String, Value>,
    children: Vec<Value>,
    next_child: Option<roxmltree::Node<'a, 'input>>,
}

impl<'a, 'input> XmlFrame<'a, 'input> {
    /// 由节点建立初始帧（tag + 可选 attrs）。
    fn new(node: roxmltree::Node<'a, 'input>) -> Self {
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
        Self {
            object,
            children: Vec::new(),
            next_child: node.first_child(),
        }
    }

    /// 收尾：装入 children 并产出最终 Value。
    fn finish(mut self) -> Value {
        if !self.children.is_empty() {
            self.object
                .insert("children".to_owned(), Value::Array(self.children));
        }
        Value::Object(self.object)
    }
}

/// 把 roxmltree 节点树转成等价 JSON 对象树。
///
/// 用**显式栈**迭代，深度不受调用栈限制（tokenizer 的递归已由
/// `max_xml_depth` 预检挡住）。
fn xml_node_to_value(root: roxmltree::Node<'_, '_>) -> Value {
    let mut stack: Vec<XmlFrame<'_, '_>> = vec![XmlFrame::new(root)];
    while let Some(frame) = stack.last_mut() {
        if let Some(child) = frame.next_child {
            frame.next_child = child.next_sibling();
            if child.is_element() {
                stack.push(XmlFrame::new(child));
            } else if child.is_text()
                && let Some(text) = child.text()
                && !text.trim().is_empty()
            {
                frame.children.push(Value::String(text.to_owned()));
            }
        } else {
            let finished = stack.pop().expect("frame");
            let value = finished.finish();
            match stack.last_mut() {
                Some(parent) => parent.children.push(value),
                None => return value,
            }
        }
    }
    Value::Null
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
