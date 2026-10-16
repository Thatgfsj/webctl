//! 表单模型与提交编码（PLAN v0.4 §2「会话」域，PR 04）。
//!
//! 覆盖：字段类型映射、默认值/隐藏字段保留、必填校验、未知道具拒绝、
//! application/x-www-form-urlencoded 与 multipart/form-data 两种编码。
//!
//! 边界（诚实声明）：按 HTML 表单提交语义编码——只有 checked 的 checkbox/radio
//! 才提交，同名多值（多 checkbox/radio、同名 text、select multiple）按控件出现
//! 顺序全部提交；不实现 datalist、不做客户端约束验证（pattern/min/max）。
//! 完整表单抽取属于 PR 06（FORM-09 多表单页选择），本模块提供 form_index。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use webctl_core::ErrorCode;

use super::html::{self, Node};

/// 表单提交方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FormMethod {
    /// GET：字段进入查询串。
    Get,
    /// POST：字段进入请求体（默认）。
    #[default]
    Post,
}

/// 表单编码方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FormEnctype {
    /// application/x-www-form-urlencoded（默认）。
    #[default]
    UrlEncoded,
    /// multipart/form-data（含文件时必须）。
    Multipart,
    /// text/plain（HTML 合法但极少使用，按 urlencoded 语义保留）。
    TextPlain,
}

/// 字段类型（由 input[type] / textarea / select 映射）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    /// 文本类（text/search/email/tel/url/number/date 等）。
    Text,
    /// 密码。
    Password,
    /// 隐藏字段。
    Hidden,
    /// 复选框。
    Checkbox,
    /// 单选框。
    Radio,
    /// 文件。
    File,
    /// 提交按钮。
    Submit,
    /// 普通按钮（不提交）。
    Button,
    /// 重置按钮。
    Reset,
    /// 图像提交按钮。
    Image,
    /// 下拉框。
    Select,
    /// 多行文本。
    Textarea,
    /// 未知类型；按文本处理但保留原始 type 供诊断。
    Other,
}

impl FieldKind {
    /// 由 input 的 type 属性映射；未知类型落到 FieldKind::Other。
    pub fn from_input_type(raw: Option<&str>) -> Self {
        match raw.unwrap_or("text").trim().to_ascii_lowercase().as_str() {
            "text" | "search" | "email" | "tel" | "url" | "number" | "date" | "time"
            | "datetime-local" | "month" | "week" | "color" | "" => Self::Text,
            "password" => Self::Password,
            "hidden" => Self::Hidden,
            "checkbox" => Self::Checkbox,
            "radio" => Self::Radio,
            "file" => Self::File,
            "submit" => Self::Submit,
            "button" => Self::Button,
            "reset" => Self::Reset,
            "image" => Self::Image,
            _ => Self::Other,
        }
    }

    /// 稳定字符串（诊断用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Password => "password",
            Self::Hidden => "hidden",
            Self::Checkbox => "checkbox",
            Self::Radio => "radio",
            Self::File => "file",
            Self::Submit => "submit",
            Self::Button => "button",
            Self::Reset => "reset",
            Self::Image => "image",
            Self::Select => "select",
            Self::Textarea => "textarea",
            Self::Other => "other",
        }
    }

    fn is_text_like(&self) -> bool {
        matches!(
            self,
            Self::Text
                | Self::Password
                | Self::Hidden
                | Self::Other
                | Self::Textarea
                | Self::Select
        )
    }

    fn is_submit_like(&self) -> bool {
        matches!(self, Self::Submit | Self::Image)
    }
}

/// 一个 select 选项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectOption {
    /// 提交值（无 value 属性时取文本）。
    pub value: String,
    /// 展示文本。
    pub label: String,
    /// 是否默认选中。
    pub selected: bool,
    /// 是否禁用。
    pub disabled: bool,
}

/// 一个表单字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    /// 字段名（无 name 的字段不参与提交，但保留在结构里）。
    pub name: String,
    /// 映射后的类型。
    pub kind: FieldKind,
    /// 原始 type 属性（诊断）。
    pub raw_type: Option<String>,
    /// 当前值（复选框/单选框为 value 属性）。
    pub value: String,
    /// 复选框/单选框默认是否选中。
    pub checked: bool,
    /// 是否禁用（禁用字段不提交）。
    pub disabled: bool,
    /// 是否必填。
    pub required: bool,
    /// select 是否允许多选。
    pub multiple: bool,
    /// select 的选项。
    pub options: Vec<SelectOption>,
}

impl Field {
    fn new(name: String, kind: FieldKind) -> Self {
        Self {
            name,
            kind,
            raw_type: None,
            value: String::new(),
            checked: false,
            disabled: false,
            required: false,
            multiple: false,
            options: Vec::new(),
        }
    }

    /// 该字段是否会参与提交（有 name、未被禁用、非普通按钮/重置）。
    pub fn is_submittable(&self) -> bool {
        !self.name.is_empty()
            && !self.disabled
            && !matches!(self.kind, FieldKind::Button | FieldKind::Reset)
    }

    /// 该字段是否为文本类（含隐藏、select、textarea）。
    pub fn is_text_like(&self) -> bool {
        self.kind.is_text_like()
    }

    /// select 的默认提交值集合（HTML 语义）：
    /// - 全部 selected 且未禁用的选项（multiple 会全部提交，单值 select 通常只有一个）；
    /// - 若没有任何 selected：单值 select 取第一个非禁用选项，multiple 则为空。
    pub fn default_select_values(&self) -> Vec<String> {
        let selected: Vec<String> = self
            .options
            .iter()
            .filter(|option| option.selected && !option.disabled)
            .map(|option| option.value.clone())
            .collect();
        if !selected.is_empty() {
            return selected;
        }
        if self.multiple {
            return Vec::new();
        }
        self.options
            .iter()
            .find(|option| !option.disabled)
            .map(|option| vec![option.value.clone()])
            .unwrap_or_default()
    }

    /// select 的默认值：取 default_select_values 的第一个。
    pub fn default_select_value(&self) -> String {
        self.default_select_values()
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    /// 该 select 是否存在某提交值。
    fn has_option(&self, value: &str) -> bool {
        self.options.iter().any(|option| option.value == value)
    }
}

/// 一个解析出的表单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Form {
    /// 在页面中的顺序（从 0 开始，稳定寻址 form_index）。
    pub index: usize,
    /// 提交方法。
    pub method: FormMethod,
    /// 原始 action（可能是相对路径、空串）。
    pub action: String,
    /// 编码方式。
    pub enctype: FormEnctype,
    /// 字段（文档顺序）。
    pub fields: Vec<Field>,
    /// id 属性。
    pub id: Option<String>,
    /// name 属性。
    pub name: Option<String>,
}

impl Form {
    /// 按名字找字段。
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.name == name)
    }

    /// 是否存在某字段。
    pub fn has_field(&self, name: &str) -> bool {
        self.field(name).is_some()
    }

    /// 全部字段名。
    pub fn field_names(&self) -> Vec<&str> {
        self.fields
            .iter()
            .filter(|field| !field.name.is_empty())
            .map(|field| field.name.as_str())
            .collect()
    }

    /// 是否为登录表单：含密码字段。
    pub fn looks_like_login(&self) -> bool {
        self.fields
            .iter()
            .any(|field| field.kind == FieldKind::Password)
    }

    /// 取默认填充：文本/隐藏/select/textarea 带默认值，复选框/单选框带默认勾选。
    pub fn default_fill(&self) -> FormFill {
        let mut fill = FormFill::new();
        for field in &self.fields {
            if field.name.is_empty() {
                continue;
            }
            match field.kind {
                // 复选框/单选框按控件出现顺序记录各自的勾选状态，
                // 从而保留同名多控件的独立状态（不再被后者覆盖）。
                FieldKind::Checkbox | FieldKind::Radio => {
                    fill.checks
                        .entry(field.name.clone())
                        .or_default()
                        .push(field.checked);
                }
                // 同名 text 也按出现顺序记录，各自保留自己的默认值。
                FieldKind::Select => {
                    let values = field.default_select_values();
                    fill.values
                        .entry(field.name.clone())
                        .or_default()
                        .extend(values);
                }
                FieldKind::File => {}
                FieldKind::Submit | FieldKind::Image | FieldKind::Button | FieldKind::Reset => {
                    fill.values
                        .entry(field.name.clone())
                        .or_default()
                        .push(field.value.clone());
                }
                _ => {
                    fill.values
                        .entry(field.name.clone())
                        .or_default()
                        .push(field.value.clone());
                }
            }
        }
        fill
    }

    /// 校验并编码为可提交的请求体。
    pub fn encode(&self, fill: &FormFill) -> Result<EncodedBody, FormError> {
        self.validate(fill)?;
        if self.enctype != FormEnctype::Multipart
            && let Some((name, _)) = fill.files.iter().next()
        {
            return Err(FormError::FileNeedsMultipart(name.clone()));
        }
        match self.enctype {
            FormEnctype::Multipart => Ok(self.encode_multipart(fill)),
            FormEnctype::UrlEncoded | FormEnctype::TextPlain => Ok(self.encode_urlencoded(fill)),
        }
    }

    fn validate(&self, fill: &FormFill) -> Result<(), FormError> {
        let names: BTreeSet<&str> = self
            .fields
            .iter()
            .filter(|field| !field.name.is_empty())
            .map(|field| field.name.as_str())
            .collect();
        for name in fill
            .values
            .keys()
            .chain(fill.checks.keys())
            .chain(fill.radios.keys())
            .chain(fill.files.keys())
        {
            if !names.contains(name.as_str()) {
                return Err(FormError::UnknownField(name.clone()));
            }
        }
        for field in &self.fields {
            if field.kind == FieldKind::Select {
                let values = self.resolve_select_values(field, fill);
                if !field.multiple && values.len() > 1 {
                    return Err(FormError::InvalidSelectOption {
                        name: field.name.clone(),
                        value: values[1].clone(),
                    });
                }
                if let Some(value) = values
                    .iter()
                    .find(|value| !value.is_empty() && !field.has_option(value))
                {
                    return Err(FormError::InvalidSelectOption {
                        name: field.name.clone(),
                        value: value.clone(),
                    });
                }
            }
            if !field.required || field.disabled || field.name.is_empty() {
                continue;
            }
            let satisfied = match field.kind {
                FieldKind::Checkbox | FieldKind::Radio => self.any_checked(&field.name, fill),
                FieldKind::File => fill.files.contains_key(&field.name),
                FieldKind::Select => !self.resolve_select_values(field, fill).is_empty(),
                _ => {
                    let values = fill
                        .values
                        .get(&field.name)
                        .map(Vec::as_slice)
                        .filter(|values| !values.is_empty());
                    match values {
                        Some(values) => values.iter().all(|value| !value.is_empty()),
                        None => !field.value.is_empty(),
                    }
                }
            };
            if !satisfied {
                return Err(FormError::MissingRequired(field.name.clone()));
            }
        }
        Ok(())
    }

    /// 名字 name 下第 index 个可提交控件最终是否勾选（validate 与 pairs 共用）。
    fn checked_at(&self, name: &str, index: usize, total: usize, fill: &FormFill) -> bool {
        if let Some(chosen) = fill.radios.get(name) {
            let radio = self
                .submittable_fields(name)
                .nth(index)
                .filter(|field| field.kind == FieldKind::Radio);
            return radio.is_some_and(|field| field.value == *chosen);
        }
        match resolve_positional(fill.checks.get(name).map(Vec::as_slice), index, total) {
            Some(checked) => checked,
            None => self
                .submittable_fields(name)
                .nth(index)
                .is_some_and(|field| field.checked),
        }
    }

    /// 同名控件是否至少有一个最终勾选（required 校验）。
    fn any_checked(&self, name: &str, fill: &FormFill) -> bool {
        let total = self.submittable_count(name);
        (0..total).any(|index| self.checked_at(name, index, total, fill))
    }

    fn submittable_fields<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Field> + 'a {
        self.fields
            .iter()
            .filter(move |field| field.name == name && field.is_submittable())
    }

    fn submittable_count(&self, name: &str) -> usize {
        self.submittable_fields(name).count()
    }

    /// 文本类控件第 index 个的最终值（同名多值按出现顺序）。
    fn text_value(&self, field: &Field, index: usize, total: usize, fill: &FormFill) -> String {
        resolve_positional(
            fill.values.get(&field.name).map(Vec::as_slice),
            index,
            total,
        )
        .unwrap_or_else(|| field.value.clone())
    }

    /// select 的最终提交值集合（显式填充优先，否则字段自身默认）。
    fn resolve_select_values(&self, field: &Field, fill: &FormFill) -> Vec<String> {
        match fill.values.get(&field.name) {
            Some(values) if !values.is_empty() => {
                if field.multiple {
                    values.clone()
                } else {
                    vec![values[0].clone()]
                }
            }
            _ => field.default_select_values(),
        }
    }

    fn encode_urlencoded(&self, fill: &FormFill) -> EncodedBody {
        let pairs = self.pairs(fill);
        let body = pairs
            .into_iter()
            .map(|(name, value)| {
                format!(
                    "{}={}",
                    percent_encode(&name, true),
                    percent_encode(&value, true)
                )
            })
            .collect::<Vec<_>>()
            .join("&");
        EncodedBody {
            content_type: "application/x-www-form-urlencoded".to_owned(),
            bytes: body.into_bytes(),
        }
    }

    fn encode_multipart(&self, fill: &FormFill) -> EncodedBody {
        let boundary = multipart_boundary();
        let mut body: Vec<u8> = Vec::new();
        for (name, value) in self.pairs(fill) {
            push_part_header(&mut body, &boundary, &name, None, None);
            body.extend_from_slice(value.as_bytes());
            body.extend_from_slice(b"\r\n");
        }
        for (name, upload) in &fill.files {
            if !self.has_field(name) {
                continue;
            }
            push_part_header(
                &mut body,
                &boundary,
                name,
                Some(upload.filename.as_str()),
                Some(upload.content_type.as_str()),
            );
            body.extend_from_slice(&upload.bytes);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        EncodedBody {
            content_type: format!("multipart/form-data; boundary={boundary}"),
            bytes: body,
        }
    }

    /// 生成 (name, value) 对，保留文档顺序与 HTML 表单语义。
    ///
    /// - 只有 checked 的 checkbox/radio 才提交，radio 组最多提交一个；
    /// - 同名多控件按出现顺序各自取值并全部提交（不再按名字折叠成一个值）；
    /// - select multiple 提交全部 selected 选项。
    fn pairs(&self, fill: &FormFill) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = Vec::new();
        let mut seen_radio: BTreeSet<&str> = BTreeSet::new();
        let mut submit_taken = false;
        let mut index_by_name: BTreeMap<&str, usize> = BTreeMap::new();
        for field in &self.fields {
            if !field.is_submittable() {
                continue;
            }
            let name = field.name.as_str();
            let index = {
                let counter = index_by_name.entry(name).or_insert(0);
                let current = *counter;
                *counter += 1;
                current
            };
            let total = self.submittable_count(name);
            match field.kind {
                FieldKind::File => {}
                FieldKind::Radio => {
                    if seen_radio.contains(name) {
                        continue;
                    }
                    if self.checked_at(name, index, total, fill) {
                        seen_radio.insert(name);
                        pairs.push((field.name.clone(), field.value.clone()));
                    }
                }
                FieldKind::Checkbox => {
                    if self.checked_at(name, index, total, fill) {
                        let value = fill
                            .values
                            .get(name)
                            .and_then(|values| values.first())
                            .filter(|value| !value.is_empty())
                            .cloned()
                            .unwrap_or_else(|| field.value.clone());
                        pairs.push((field.name.clone(), value));
                    }
                }
                FieldKind::Select => {
                    for value in self.resolve_select_values(field, fill) {
                        pairs.push((field.name.clone(), value));
                    }
                }
                kind if kind.is_submit_like() => {
                    if submit_taken {
                        continue;
                    }
                    submit_taken = true;
                    let value = fill
                        .values
                        .get(name)
                        .and_then(|values| values.first())
                        .cloned()
                        .unwrap_or_else(|| field.value.clone());
                    pairs.push((field.name.clone(), value));
                }
                _ => {
                    let value = self.text_value(field, index, total, fill);
                    pairs.push((field.name.clone(), value));
                }
            }
        }
        pairs
    }
}

fn push_part_header(
    body: &mut Vec<u8>,
    boundary: &str,
    name: &str,
    filename: Option<&str>,
    content_type: Option<&str>,
) {
    let mut header = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"");
    if let Some(filename) = filename {
        let _ = write!(header, "; filename=\"{filename}\"");
    }
    header.push_str("\r\n");
    if let Some(content_type) = content_type {
        let _ = write!(header, "Content-Type: {content_type}\r\n");
    }
    header.push_str("\r\n");
    body.extend_from_slice(header.as_bytes());
}

fn multipart_boundary() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("----webctl{nanos:032x}{count:08x}")
}

/// 一次表单填充：值、勾选状态、radio 选择与文件。
///
/// 值为每名字一个列表，按控件出现顺序对齐（default_fill 会为同名多值控件
/// 逐个填入默认值）。只设置一个值时视为对该名字的所有同名控件生效。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormFill {
    values: BTreeMap<String, Vec<String>>,
    checks: BTreeMap<String, Vec<bool>>,
    radios: BTreeMap<String, String>,
    files: BTreeMap<String, FileUpload>,
}

impl FormFill {
    /// 空填充。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置文本类字段（含隐藏、select、textarea、提交按钮覆盖）。
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.values.insert(name.into(), vec![value.into()]);
        self
    }

    /// 按控件出现顺序设置同名多值（同名多个 text / select multiple）。
    pub fn set_many<I, V>(&mut self, name: impl Into<String>, values: I) -> &mut Self
    where
        I: IntoIterator<Item = V>,
        V: Into<String>,
    {
        self.values
            .insert(name.into(), values.into_iter().map(Into::into).collect());
        self
    }

    /// 设置复选框勾选状态；对所有同名控件生效。
    pub fn check(&mut self, name: impl Into<String>, checked: bool) -> &mut Self {
        self.checks.insert(name.into(), vec![checked]);
        self
    }

    /// 设置同名多复选框中第 index 个的勾选状态（按出现顺序）。
    pub fn check_at(&mut self, name: impl Into<String>, index: usize, checked: bool) -> &mut Self {
        let entry = self.checks.entry(name.into()).or_default();
        if entry.len() <= index {
            entry.resize(index + 1, false);
        }
        entry[index] = checked;
        self
    }

    /// 选中某个单选框（按 value 匹配该名字下的 radio）。
    pub fn select_radio(&mut self, name: impl Into<String>, value: impl Into<String>) -> &mut Self {
        let name = name.into();
        let value = value.into();
        self.values.insert(name.clone(), vec![value.clone()]);
        self.radios.insert(name, value);
        self
    }

    /// 指定文件上传。
    pub fn upload(&mut self, name: impl Into<String>, file: FileUpload) -> &mut Self {
        self.files.insert(name.into(), file);
        self
    }

    /// 合并另一份填充：同名值、勾选、radio 选择与文件由 other 覆盖。
    pub fn merge(&mut self, other: &FormFill) {
        for (name, values) in &other.values {
            self.values.insert(name.clone(), values.clone());
        }
        for (name, checks) in &other.checks {
            self.checks.insert(name.clone(), checks.clone());
        }
        for (name, value) in &other.radios {
            self.radios.insert(name.clone(), value.clone());
        }
        for (name, file) in &other.files {
            self.files.insert(name.clone(), file.clone());
        }
    }

    /// 是否对该名字设置了值/勾选/radio/文件。
    pub fn contains(&self, name: &str) -> bool {
        self.values.contains_key(name)
            || self.checks.contains_key(name)
            || self.radios.contains_key(name)
            || self.files.contains_key(name)
    }

    /// 读取已设置的第一个值。
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .get(name)
            .and_then(|values| values.first())
            .map(String::as_str)
    }
}

/// 文件上传载荷。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileUpload {
    /// 文件名。
    pub filename: String,
    /// 内容类型。
    pub content_type: String,
    /// 文件字节。
    pub bytes: Vec<u8>,
}

impl FileUpload {
    /// 构造一个上传。
    pub fn new(
        filename: impl Into<String>,
        content_type: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            filename: filename.into(),
            content_type: content_type.into(),
            bytes: bytes.into(),
        }
    }
}

/// 编码后的请求体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedBody {
    /// Content-Type 头值。
    pub content_type: String,
    /// 请求体字节。
    pub bytes: Vec<u8>,
}

/// 表单填充/编码错误（映射到封闭错误码）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormError {
    /// 填充了表单里不存在的字段。
    UnknownField(String),
    /// 必填字段缺失。
    MissingRequired(String),
    /// 文件上传要求 multipart enctype。
    FileNeedsMultipart(String),
    /// select 提交了不存在的选项。
    InvalidSelectOption {
        /// 字段名。
        name: String,
        /// 非法值。
        value: String,
    },
}

impl FormError {
    /// 映射到结果信封错误码。
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::UnknownField(_) | Self::MissingRequired(_) | Self::InvalidSelectOption { .. } => {
                ErrorCode::Parse
            }
            Self::FileNeedsMultipart(_) => ErrorCode::Unsupported,
        }
    }

    /// 人类可读说明。
    pub fn message(&self) -> String {
        match self {
            Self::UnknownField(name) => {
                format!("form has no field named {name:?}; refusing to silently ignore it")
            }
            Self::MissingRequired(name) => format!("required field {name:?} is empty"),
            Self::FileNeedsMultipart(name) => format!(
                "file field {name:?} requires multipart/form-data; refusing urlencoded upload"
            ),
            Self::InvalidSelectOption { name, value } => {
                format!("select field {name:?} has no option with value {value:?}")
            }
        }
    }
}

impl std::fmt::Display for FormError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// 解析页面里的全部表单。
pub fn parse_forms(html: &str) -> Vec<Form> {
    let nodes = html::tokenize(html);
    let mut forms: Vec<Form> = Vec::new();
    let mut inside_form = false;
    let mut select_field: Option<usize> = None;
    let mut textarea_field: Option<usize> = None;
    let mut inside_option = false;

    for node in &nodes {
        match node {
            Node::Tag(tag) if tag.name == "form" => {
                if tag.closing {
                    inside_form = false;
                    select_field = None;
                    textarea_field = None;
                } else {
                    inside_form = true;
                    let method = match tag.attr("method") {
                        Some(value) if value.eq_ignore_ascii_case("get") => FormMethod::Get,
                        _ => FormMethod::Post,
                    };
                    let enctype = match tag.attr("enctype") {
                        Some(value) if value.eq_ignore_ascii_case("multipart/form-data") => {
                            FormEnctype::Multipart
                        }
                        Some(value) if value.eq_ignore_ascii_case("text/plain") => {
                            FormEnctype::TextPlain
                        }
                        _ => FormEnctype::UrlEncoded,
                    };
                    forms.push(Form {
                        index: forms.len(),
                        method,
                        action: tag.attr("action").unwrap_or_default().to_owned(),
                        enctype,
                        fields: Vec::new(),
                        id: tag.attr("id").map(str::to_owned),
                        name: tag.attr("name").map(str::to_owned),
                    });
                }
            }
            Node::Tag(tag) if inside_form && tag.name == "input" && !tag.closing => {
                let Some(form) = forms.last_mut() else {
                    continue;
                };
                let raw_type = tag.attr("type").map(str::to_owned);
                let kind = FieldKind::from_input_type(raw_type.as_deref());
                let mut field = Field::new(tag.attr("name").unwrap_or_default().to_owned(), kind);
                field.raw_type = raw_type;
                field.value = tag.attr("value").unwrap_or_default().to_owned();
                if matches!(field.kind, FieldKind::Checkbox | FieldKind::Radio)
                    && field.value.is_empty()
                {
                    field.value = "on".to_owned();
                }
                field.checked = tag.has_attr("checked");
                field.disabled = tag.has_attr("disabled");
                field.required = tag.has_attr("required");
                form.fields.push(field);
            }
            Node::Tag(tag) if inside_form && tag.name == "textarea" && tag.closing => {
                textarea_field = None;
            }
            Node::Tag(tag) if inside_form && tag.name == "textarea" && !tag.closing => {
                let Some(form) = forms.last_mut() else {
                    continue;
                };
                let mut field = Field::new(
                    tag.attr("name").unwrap_or_default().to_owned(),
                    FieldKind::Textarea,
                );
                field.disabled = tag.has_attr("disabled");
                field.required = tag.has_attr("required");
                textarea_field = Some(form.fields.len());
                form.fields.push(field);
            }
            Node::Tag(tag) if inside_form && tag.name == "select" => {
                let Some(form) = forms.last_mut() else {
                    continue;
                };
                if tag.closing {
                    select_field = None;
                    inside_option = false;
                } else {
                    let mut field = Field::new(
                        tag.attr("name").unwrap_or_default().to_owned(),
                        FieldKind::Select,
                    );
                    field.multiple = tag.has_attr("multiple");
                    field.disabled = tag.has_attr("disabled");
                    field.required = tag.has_attr("required");
                    select_field = Some(form.fields.len());
                    form.fields.push(field);
                }
            }
            Node::Tag(tag) if inside_form && tag.name == "option" && tag.closing => {
                inside_option = false;
            }
            Node::Tag(tag) if inside_form && tag.name == "option" && !tag.closing => {
                inside_option = true;
                let Some(index) = select_field else { continue };
                let Some(form) = forms.last_mut() else {
                    continue;
                };
                let Some(field) = form.fields.get_mut(index) else {
                    continue;
                };
                field.options.push(SelectOption {
                    value: tag.attr("value").unwrap_or_default().to_owned(),
                    label: String::new(),
                    selected: tag.has_attr("selected"),
                    disabled: tag.has_attr("disabled"),
                });
            }
            Node::Tag(tag) if inside_form && tag.name == "button" && !tag.closing => {
                let Some(form) = forms.last_mut() else {
                    continue;
                };
                let raw_type = tag.attr("type").map(str::to_owned);
                let kind = match raw_type.as_deref() {
                    Some(value) if value.eq_ignore_ascii_case("submit") => FieldKind::Submit,
                    Some(value) if value.eq_ignore_ascii_case("reset") => FieldKind::Reset,
                    _ => FieldKind::Button,
                };
                let mut field = Field::new(tag.attr("name").unwrap_or_default().to_owned(), kind);
                field.raw_type = raw_type;
                field.value = tag.attr("value").unwrap_or_default().to_owned();
                field.disabled = tag.has_attr("disabled");
                form.fields.push(field);
            }
            Node::Text(text) => {
                if let Some(index) = textarea_field
                    && let Some(form) = forms.last_mut()
                    && let Some(field) = form.fields.get_mut(index)
                {
                    field.value.push_str(text);
                    textarea_field = None;
                }
                if inside_option
                    && let Some(index) = select_field
                    && let Some(form) = forms.last_mut()
                    && let Some(field) = form.fields.get_mut(index)
                    && let Some(option) = field.options.last_mut()
                {
                    option.label.push_str(text);
                }
            }
            _ => {}
        }
    }

    for form in &mut forms {
        for field in &mut form.fields {
            if field.kind == FieldKind::Select {
                for option in &mut field.options {
                    if option.value.is_empty() {
                        option.value = option.label.clone();
                    }
                }
                if field.value.is_empty() {
                    field.value = field.default_select_value();
                }
            }
        }
    }
    forms
}

/// 从按名字保存的取值列表中解析第 index 个位置的值。
///
/// - 列表只有 1 个元素：视为广播到所有同名控件；
/// - 否则按位置取，越界时退回最后一个（部分显式填充不至于丢值）。
fn resolve_positional<T: Clone>(list: Option<&[T]>, index: usize, total: usize) -> Option<T> {
    let list = list?;
    if list.is_empty() {
        return None;
    }
    if list.len() == 1 {
        return Some(list[0].clone());
    }
    if list.len() >= total {
        return list.get(index).cloned();
    }
    list.get(index).or_else(|| list.last()).cloned()
}

fn percent_encode(input: &str, space_as_plus: bool) -> String {
    let mut output = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'*' => {
                output.push(*byte as char);
            }
            b' ' if space_as_plus => output.push('+'),
            _ => {
                let _ = write!(output, "%{byte:02X}");
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_form(html: &str) -> Form {
        parse_forms(html).into_iter().next().expect("form")
    }

    #[test]
    fn maps_input_types_and_hidden_fields() {
        let html = r#"<form action="/f" method="post">
                 <input type="hidden" name="csrf" value="tok">
                 <input type="text" name="user" value="默认">
                 <input type="checkbox" name="ok" checked>
                 <input type="radio" name="r" value="a">
                 <input type="radio" name="r" value="b" checked>
                 <select name="s"><option value="x">X</option><option value="y" selected>Y</option></select>
                 <textarea name="note">hello</textarea>
                 <input type="unknown" name="weird" value="v">
               </form>"#;
        let form = first_form(html);
        assert_eq!(form.method, FormMethod::Post);
        assert_eq!(form.field("csrf").unwrap().kind, FieldKind::Hidden);
        assert_eq!(form.field("user").unwrap().value, "默认");
        assert_eq!(form.field("ok").unwrap().value, "on");
        assert!(form.field("ok").unwrap().checked);
        assert_eq!(form.field("s").unwrap().default_select_value(), "y");
        assert_eq!(form.field("note").unwrap().value, "hello");
        assert_eq!(form.field("weird").unwrap().kind, FieldKind::Other);
    }

    #[test]
    fn urlencoded_encoding_preserves_order_and_utf8() {
        let html = r#"<form action="/f"><input type="hidden" name="csrf" value="t1"><input name="q" value="中文 a&b"></form>"#;
        let form = first_form(html);
        let mut fill = form.default_fill();
        fill.set("q", "中文 a&b");
        let body = form.encode(&fill).expect("encode");
        assert_eq!(body.content_type, "application/x-www-form-urlencoded");
        assert_eq!(
            String::from_utf8(body.bytes).expect("utf8"),
            "csrf=t1&q=%E4%B8%AD%E6%96%87+a%26b"
        );
    }

    #[test]
    fn unknown_field_and_missing_required_are_errors() {
        let form =
            first_form(r#"<form action="/f"><input name="a" required><input name="b"></form>"#);
        let mut fill = form.default_fill();
        fill.set("nope", "1");
        assert_eq!(
            form.encode(&fill),
            Err(FormError::UnknownField("nope".to_owned()))
        );
        let mut fill = form.default_fill();
        fill.set("b", "x");
        assert_eq!(
            form.encode(&fill),
            Err(FormError::MissingRequired("a".to_owned()))
        );
    }

    #[test]
    fn checkbox_and_radio_follow_html_semantics() {
        let html = r#"<form action="/f">
                 <input type="checkbox" name="c" value="yes" checked>
                 <input type="checkbox" name="d" value="no">
                 <input type="radio" name="r" value="a">
                 <input type="radio" name="r" value="b" checked>
               </form>"#;
        let form = first_form(html);
        let fill = form.default_fill();
        let body = form.encode(&fill).expect("encode");
        assert_eq!(String::from_utf8(body.bytes).expect("utf8"), "c=yes&r=b");

        let mut fill = form.default_fill();
        fill.check("c", false).select_radio("r", "a");
        let body = form.encode(&fill).expect("encode");
        assert_eq!(String::from_utf8(body.bytes).expect("utf8"), "r=a");
    }

    #[test]
    fn multipart_includes_file_bytes() {
        let html = r#"<form action="/up" method="post" enctype="multipart/form-data"><input type="file" name="doc"></form>"#;
        let form = first_form(html);
        let mut fill = form.default_fill();
        fill.upload(
            "doc",
            FileUpload::new("a.txt", "text/plain", b"hello".to_vec()),
        );
        let body = form.encode(&fill).expect("encode");
        assert!(
            body.content_type
                .starts_with("multipart/form-data; boundary=")
        );
        let text = String::from_utf8_lossy(&body.bytes);
        assert!(text.contains("filename=\"a.txt\""));
        assert!(text.contains("Content-Type: text/plain"));
        assert!(text.contains("hello"));
    }

    #[test]
    fn file_on_urlencoded_form_is_unsupported() {
        let form = first_form(r#"<form action="/up"><input type="file" name="doc"></form>"#);
        let mut fill = form.default_fill();
        fill.upload("doc", FileUpload::new("a.txt", "text/plain", b"x".to_vec()));
        assert_eq!(
            form.encode(&fill),
            Err(FormError::FileNeedsMultipart("doc".to_owned()))
        );
    }

    #[test]
    fn duplicate_checkbox_same_name_submits_only_checked() {
        let first = first_form(
            r#"<form action="/f">
                 <input type="checkbox" name="tags" value="a" checked>
                 <input type="checkbox" name="tags" value="b">
               </form>"#,
        );
        let body = first.encode(&first.default_fill()).expect("encode");
        assert_eq!(String::from_utf8(body.bytes).expect("utf8"), "tags=a");

        let second = first_form(
            r#"<form action="/f">
                 <input type="checkbox" name="tags" value="a">
                 <input type="checkbox" name="tags" value="b" checked>
               </form>"#,
        );
        let body = second.encode(&second.default_fill()).expect("encode");
        assert_eq!(String::from_utf8(body.bytes).expect("utf8"), "tags=b");
    }

    #[test]
    fn duplicate_text_inputs_keep_document_order() {
        let form = first_form(
            r#"<form action="/f"><input name="q" value="one"><input name="q" value="two"></form>"#,
        );
        let body = form.encode(&form.default_fill()).expect("encode");
        assert_eq!(String::from_utf8(body.bytes).expect("utf8"), "q=one&q=two");

        // 显式只设一个值时广播到所有同名控件。
        let mut fill = form.default_fill();
        fill.set("q", "x");
        let body = form.encode(&fill).expect("encode");
        assert_eq!(String::from_utf8(body.bytes).expect("utf8"), "q=x&q=x");
    }

    #[test]
    fn select_multiple_submits_all_selected() {
        let form = first_form(
            r#"<form action="/f"><select name="colors" multiple>
                 <option value="red">Red</option>
                 <option value="green" selected>Green</option>
                 <option value="blue" selected>Blue</option>
               </select></form>"#,
        );
        let body = form.encode(&form.default_fill()).expect("encode");
        assert_eq!(
            String::from_utf8(body.bytes).expect("utf8"),
            "colors=green&colors=blue"
        );
    }
}
