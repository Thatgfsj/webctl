//! 内置 JSON 查询子集（PLAN v0.4 §3.7 工具 11，替代外部 jq）。
//!
//! 这是**有意收窄的子集**，不追求 jq 的完整语义。语法如下（EBNF 风格）：
//!
//!     query       := [ '$' ] step*
//!     step        := '.' name                       // 取对象成员
//!                  | '[' index ']'                  // 取数组下标（从 0 开始）
//!                  | '[' '*' ']'                    // 展开数组全部元素
//!                  | '[' '?' pred ']'               // 过滤数组元素
//!                  | '.{' name (',' name)* '}'      // 投影：只保留列出的字段
//!     pred        := [ '(' ] [ '@' ] path op literal [ ')' ]
//!     path        := ( '.' name | '[' index ']' )*
//!     op          := '==' | '!=' | '<' | '<=' | '>' | '>='
//!     literal     := number | string | 'true' | 'false' | 'null'
//!     name        := identifier | '"' ... '"' | "'" ... "'"
//!
//! 语义（写死，测试与文档一致）：
//!
//! 1. 求值起点是根值；每一步作用于当前选中的**一组**节点。
//! 2. [index] 越界、对非对象取成员、对非数组取下标、对非对象投影，都返回
//!    PARSE 错误——**不**把类型错误伪装成 null 成功。
//! 3. 对象缺少某成员时取值为 null（jq 语义），这不是错误。
//! 4. 只要用了 [*] 或 [?...]，结果就是数组（即使只命中一个元素，或一个都没命中）。
//!    没有集合步骤时，结果是单个值；根路径 $ 返回整个文档。
//! 5. 过滤谓词里的比较：两侧都是数字按数值比较，都是字符串按字典序比较，
//!    其它类型组合一律为 false（除了 == / !=）。
//! 6. 表达式语法非法、输入不是合法 JSON、或违反上述语义规则，都返回 code=PARSE 且 ok=false。
//!
//! 与 jmespath 的取舍见 PLAN §9 第 3 项：本子集零新增依赖、只覆盖路径/过滤/投影三件事，
//! 不引入完整查询语言带来的语义面与依赖面。

use std::cmp::Ordering;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use webctl_core::{Envelope, ErrorCode, Trust};

/// 默认 JSON 输入上限（8 MiB）：超过直接 LIMIT，不进入解析。
pub const DEFAULT_MAX_JSON_BYTES: usize = 8 * 1024 * 1024;

/// json_query 参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonQueryOptions {
    /// 允许解析的最大输入字节数。
    pub max_bytes: usize,
}

impl Default for JsonQueryOptions {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_JSON_BYTES,
        }
    }
}

/// 查询结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonQueryOutcome {
    /// 选中的值。集合步骤产生数组；否则为单个值（可能为 null）。
    pub value: Value,
    /// 最终选中节点数。过滤零命中的空结果合法且 matched == 0。
    pub matched: usize,
}

/// 对 input 执行 query，返回统一信封。
///
/// input 为原始 JSON 字节，query 为上述子集表达式。
/// 输入超限返回 LIMIT（meta.degraded_from == "json-input-size-limit"），
/// 不读取也不算任何东西；JSON 或表达式非法返回 PARSE。
pub fn json_query(
    input: &[u8],
    query: &str,
    options: &JsonQueryOptions,
) -> Envelope<JsonQueryOutcome> {
    let started = Instant::now();
    let elapsed = || started.elapsed().as_millis() as u64;

    if input.len() > options.max_bytes {
        return Envelope::degraded(
            JsonQueryOutcome {
                value: Value::Null,
                matched: 0,
            },
            "json-input-size-limit",
            ErrorCode::Limit,
            elapsed(),
        );
    }

    let root: Value = match serde_json::from_slice(input) {
        Ok(value) => value,
        Err(error) => {
            return Envelope::fail(
                ErrorCode::Parse,
                format!("JSON 解析失败：{error}"),
                elapsed(),
            );
        }
    };

    let program = match Program::parse(query) {
        Ok(program) => program,
        Err(error) => {
            return Envelope::fail(
                ErrorCode::Parse,
                format!("查询表达式非法：{error}"),
                elapsed(),
            );
        }
    };

    match program.eval(&root) {
        Ok((value, matched)) => Envelope::ok(
            JsonQueryOutcome { value, matched },
            Trust::UntrustedPageContent,
            elapsed(),
        ),
        Err(error) => Envelope::fail(
            ErrorCode::Parse,
            format!("查询执行失败：{error}"),
            elapsed(),
        ),
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Program {
    steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq)]
enum Step {
    Member(String),
    Index(usize),
    Wildcard,
    Filter(Predicate),
    Project(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
struct Predicate {
    path: Vec<PathSeg>,
    op: Op,
    literal: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq)]
enum PathSeg {
    Member(String),
    Index(usize),
}

impl Program {
    fn parse(query: &str) -> Result<Self, String> {
        let mut parser = Parser::new(query);
        parser.skip_ws();
        parser.eat('$');
        let mut steps = Vec::new();
        loop {
            parser.skip_ws();
            if parser.at_end() {
                break;
            }
            steps.push(parser.parse_step()?);
        }
        Ok(Self { steps })
    }

    fn eval(&self, root: &Value) -> Result<(Value, usize), String> {
        let mut current: Vec<Value> = vec![root.clone()];
        let mut collection = false;

        for step in &self.steps {
            let mut next: Vec<Value> = Vec::new();
            match step {
                Step::Member(name) => {
                    for node in &current {
                        match node {
                            Value::Object(map) => {
                                next.push(map.get(name).cloned().unwrap_or(Value::Null));
                            }
                            other => {
                                return Err(format!(
                                    "对 {} 取成员 {name:?}：应为对象",
                                    type_name(other)
                                ));
                            }
                        }
                    }
                }
                Step::Index(index) => {
                    for node in &current {
                        match node {
                            Value::Array(array) => {
                                if *index < array.len() {
                                    next.push(array[*index].clone());
                                } else {
                                    return Err(format!(
                                        "数组下标 {index} 越界（长度 {}）",
                                        array.len()
                                    ));
                                }
                            }
                            other => {
                                return Err(format!(
                                    "对 {} 取下标 [{index}]：应为数组",
                                    type_name(other)
                                ));
                            }
                        }
                    }
                }
                Step::Wildcard => {
                    collection = true;
                    for node in &current {
                        match node {
                            Value::Array(array) => next.extend(array.iter().cloned()),
                            Value::Object(map) => next.extend(map.values().cloned()),
                            other => {
                                return Err(format!(
                                    "对 {} 做 [*] 展开：应为数组或对象",
                                    type_name(other)
                                ));
                            }
                        }
                    }
                }
                Step::Filter(predicate) => {
                    collection = true;
                    for node in &current {
                        match node {
                            Value::Array(array) => {
                                for item in array {
                                    if predicate.test(item) {
                                        next.push(item.clone());
                                    }
                                }
                            }
                            other => {
                                return Err(format!(
                                    "对 {} 做 [?] 过滤：应为数组",
                                    type_name(other)
                                ));
                            }
                        }
                    }
                }
                Step::Project(fields) => {
                    for node in &current {
                        match node {
                            Value::Object(map) => {
                                let mut projected = Map::new();
                                for field in fields {
                                    projected.insert(
                                        field.clone(),
                                        map.get(field).cloned().unwrap_or(Value::Null),
                                    );
                                }
                                next.push(Value::Object(projected));
                            }
                            other => {
                                return Err(format!(
                                    "对 {} 做投影 {{...}}：应为对象",
                                    type_name(other)
                                ));
                            }
                        }
                    }
                }
            }
            current = next;
        }

        let matched = current.len();
        let value = if collection || matched != 1 {
            Value::Array(current)
        } else {
            current.into_iter().next().unwrap_or(Value::Null)
        };
        Ok((value, matched))
    }
}

impl Predicate {
    fn test(&self, item: &Value) -> bool {
        let left = navigate(item, &self.path);
        compare(left, self.op, &self.literal)
    }
}

fn navigate<'a>(item: &'a Value, path: &[PathSeg]) -> Option<&'a Value> {
    let mut current = item;
    for segment in path {
        current = match segment {
            PathSeg::Member(name) => match current {
                Value::Object(map) => map.get(name)?,
                _ => return None,
            },
            PathSeg::Index(index) => match current {
                Value::Array(array) => array.get(*index)?,
                _ => return None,
            },
        };
    }
    Some(current)
}

fn compare(left: Option<&Value>, op: Op, literal: &Value) -> bool {
    let Some(left) = left else {
        return matches!(op, Op::Ne);
    };
    match op {
        Op::Eq => left == literal,
        Op::Ne => left != literal,
        Op::Lt | Op::Le | Op::Gt | Op::Ge => compare_order(left, op, literal),
    }
}

fn compare_order(left: &Value, op: Op, literal: &Value) -> bool {
    let ordering = match (left, literal) {
        (Value::Number(a), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => a.partial_cmp(&b),
            _ => None,
        },
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        _ => None,
    };
    let Some(ordering) = ordering else {
        return false;
    };
    match op {
        Op::Lt => ordering == Ordering::Less,
        Op::Le => ordering != Ordering::Greater,
        Op::Gt => ordering == Ordering::Greater,
        Op::Ge => ordering != Ordering::Less,
        _ => false,
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "布尔值",
        Value::Number(_) => "数字",
        Value::String(_) => "字符串",
        Value::Array(_) => "数组",
        Value::Object(_) => "对象",
    }
}

struct Parser<'a> {
    chars: Vec<char>,
    pos: usize,
    _source: &'a str,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
            _source: source,
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek();
        if ch.is_some() {
            self.pos += 1;
        }
        ch
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: char) -> Result<(), String> {
        if self.eat(expected) {
            Ok(())
        } else {
            Err(format!(
                "位置 {} 期望 {expected:?}，实际 {:?}",
                self.pos,
                self.peek()
            ))
        }
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn parse_step(&mut self) -> Result<Step, String> {
        match self.peek() {
            Some('.') => {
                self.bump();
                if self.eat('{') {
                    self.parse_projection()
                } else {
                    Ok(Step::Member(self.parse_name()?))
                }
            }
            Some('[') => {
                self.bump();
                self.skip_ws();
                let step = match self.peek() {
                    Some('*') => {
                        self.bump();
                        Step::Wildcard
                    }
                    Some('?') => {
                        self.bump();
                        Step::Filter(self.parse_predicate()?)
                    }
                    Some(_) => Step::Index(self.parse_index()?),
                    None => return Err("位置结尾缺少 ']'".to_string()),
                };
                self.skip_ws();
                self.expect(']')?;
                Ok(step)
            }
            other => Err(format!("位置 {} 出现非法步骤 {other:?}", self.pos)),
        }
    }

    fn parse_projection(&mut self) -> Result<Step, String> {
        let mut fields = Vec::new();
        loop {
            self.skip_ws();
            fields.push(self.parse_name()?);
            self.skip_ws();
            if !self.eat(',') {
                break;
            }
        }
        self.skip_ws();
        self.expect('}')?;
        if fields.is_empty() {
            return Err("投影字段不能为空".to_string());
        }
        Ok(Step::Project(fields))
    }

    fn parse_predicate(&mut self) -> Result<Predicate, String> {
        self.skip_ws();
        let parenthesized = self.eat('(');
        self.skip_ws();
        self.expect('@')?;
        let mut path = Vec::new();
        loop {
            self.skip_ws();
            if self.eat('.') {
                path.push(PathSeg::Member(self.parse_name()?));
            } else if self.eat('[') {
                self.skip_ws();
                let index = self.parse_index()?;
                self.skip_ws();
                self.expect(']')?;
                path.push(PathSeg::Index(index));
            } else {
                break;
            }
        }
        self.skip_ws();
        let op = self.parse_op()?;
        self.skip_ws();
        let literal = self.parse_literal()?;
        self.skip_ws();
        if parenthesized {
            self.expect(')')?;
        }
        Ok(Predicate { path, op, literal })
    }

    fn parse_op(&mut self) -> Result<Op, String> {
        if self.eat('=') {
            self.expect('=')?;
            return Ok(Op::Eq);
        }
        if self.eat('!') {
            self.expect('=')?;
            return Ok(Op::Ne);
        }
        if self.eat('<') {
            return Ok(if self.eat('=') { Op::Le } else { Op::Lt });
        }
        if self.eat('>') {
            return Ok(if self.eat('=') { Op::Ge } else { Op::Gt });
        }
        Err(format!("位置 {} 期望比较运算符", self.pos))
    }

    fn parse_index(&mut self) -> Result<usize, String> {
        let mut digits = String::new();
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            digits.push(self.bump().expect("digit"));
        }
        if digits.is_empty() {
            return Err(format!("位置 {} 期望数组下标", self.pos));
        }
        digits
            .parse::<usize>()
            .map_err(|error| format!("下标 {digits:?} 非法：{error}"))
    }

    fn parse_name(&mut self) -> Result<String, String> {
        self.skip_ws();
        match self.peek() {
            Some('"') | Some('\'') => self.parse_quoted(),
            Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == '-' => {
                let mut name = String::new();
                while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    name.push(self.bump().expect("name"));
                }
                Ok(name)
            }
            other => Err(format!("位置 {} 期望字段名，实际 {other:?}", self.pos)),
        }
    }

    fn parse_quoted(&mut self) -> Result<String, String> {
        let quote = self.bump().expect("quote");
        let mut value = String::new();
        loop {
            match self.bump() {
                None => return Err("字符串缺少收尾引号".to_string()),
                Some(ch) if ch == quote => return Ok(value),
                Some('\\') => {
                    let escaped = self.bump().ok_or("转义符后缺少字符")?;
                    let decoded = match escaped {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        '\\' => '\\',
                        '"' => '"',
                        '\'' => '\'',
                        other => other,
                    };
                    value.push(decoded);
                }
                Some(ch) => value.push(ch),
            }
        }
    }

    fn parse_literal(&mut self) -> Result<Value, String> {
        self.skip_ws();
        if matches!(self.peek(), Some('"' | '\'')) {
            return Ok(Value::String(self.parse_quoted()?));
        }
        if self.peek() == Some('-') || matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            let mut number = String::new();
            if self.peek() == Some('-') {
                number.push(self.bump().expect("minus"));
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                number.push(self.bump().expect("digit"));
            }
            if self.eat('.') {
                number.push('.');
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    number.push(self.bump().expect("digit"));
                }
            }
            if matches!(self.peek(), Some('e' | 'E')) {
                number.push(self.bump().expect("exponent"));
                if matches!(self.peek(), Some('+' | '-')) {
                    number.push(self.bump().expect("sign"));
                }
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    number.push(self.bump().expect("digit"));
                }
            }
            return serde_json::from_str::<Value>(&number)
                .map_err(|error| format!("数字字面量 {number:?} 非法：{error}"));
        }
        for (keyword, value) in [
            ("true", Value::Bool(true)),
            ("false", Value::Bool(false)),
            ("null", Value::Null),
        ] {
            if self.consume_keyword(keyword) {
                return Ok(value);
            }
        }
        Err(format!("位置 {} 期望字面量", self.pos))
    }

    fn consume_keyword(&mut self, keyword: &str) -> bool {
        let end = self.pos + keyword.chars().count();
        if end > self.chars.len() {
            return false;
        }
        let candidate: String = self.chars[self.pos..end].iter().collect();
        if candidate == keyword {
            self.pos = end;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        br#"{
          "store": {
            "book": [
              {"title": "Book A", "price": 8.5, "tags": ["a", "b"]},
              {"title": "Book B", "price": 12.0, "tags": ["b"]},
              {"title": "Book C", "price": 5.0, "tags": []}
            ],
            "open": true
          },
          "count": 3,
          "empty": []
        }"#
        .to_vec()
    }

    fn run(query: &str) -> Value {
        let env = json_query(&sample(), query, &JsonQueryOptions::default());
        assert!(env.ok, "{query}: {}", env.error);
        env.data.expect("data").value
    }

    /// DOC-06：路径取值。
    #[test]
    fn path_selection() {
        assert_eq!(run("$.store.book[0].title"), serde_json::json!("Book A"));
        assert_eq!(run("$.count"), serde_json::json!(3));
        assert_eq!(
            run("$"),
            serde_json::from_slice::<Value>(&sample()).unwrap()
        );
        // 缺少成员是 null，不是错误。
        assert_eq!(run("$.store.missing"), Value::Null);
    }

    /// DOC-07：过滤与投影。
    #[test]
    fn filter_and_projection() {
        assert_eq!(
            run("$.store.book[?(@.price < 10)].title"),
            serde_json::json!(["Book A", "Book C"])
        );
        assert_eq!(
            run("$.store.book[*].title"),
            serde_json::json!(["Book A", "Book B", "Book C"])
        );
        assert_eq!(
            run("$.store.book[*].{title, price}"),
            serde_json::json!([
                {"title": "Book A", "price": 8.5},
                {"title": "Book B", "price": 12.0},
                {"title": "Book C", "price": 5.0}
            ])
        );
        // 过滤零命中：空数组是成功，不是解析失败。
        assert_eq!(run("$.store.book[?(@.price > 100)]"), serde_json::json!([]));
    }

    /// DOC-08：非法表达式必须 PARSE，禁止把错误当 null 成功。
    #[test]
    fn invalid_expressions_error() {
        for bad in [
            "$.store.book[?(@.price <)]",
            "$.store.book[",
            "$.store.",
            "$.store.book[abc]",
            "$[?(@.price < 10)",
            "$.store.book[*].{title,",
        ] {
            let env = json_query(&sample(), bad, &JsonQueryOptions::default());
            assert!(!env.ok, "{bad:?} 不应成功");
            assert_eq!(env.code, ErrorCode::Parse, "{bad:?}");
            assert!(!env.error.is_empty(), "{bad:?}");
            assert!(env.data.is_none(), "{bad:?}");
            assert_eq!(env.validate(), Ok(()));
        }
    }

    /// DOC-10：输入超限直接 LIMIT，不解析。
    #[test]
    fn oversized_input_is_limit() {
        let options = JsonQueryOptions { max_bytes: 16 };
        let env = json_query(&sample(), "$.count", &options);
        assert!(env.ok);
        assert_eq!(env.code, ErrorCode::Limit);
        assert_eq!(
            env.meta.degraded_from.as_deref(),
            Some("json-input-size-limit")
        );
        assert_eq!(env.validate(), Ok(()));
    }

    /// 输入不是合法 JSON，返回 PARSE。
    #[test]
    fn malformed_json_errors() {
        let env = json_query(b"{ not json", "$.a", &JsonQueryOptions::default());
        assert!(!env.ok);
        assert_eq!(env.code, ErrorCode::Parse);
    }
}
