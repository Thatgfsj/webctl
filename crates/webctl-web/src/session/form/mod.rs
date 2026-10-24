//! 表单模型与提交编码（PLAN v0.4 §2「会话」域，PR 04）。
//!
//! 覆盖：字段类型映射、默认值/隐藏字段保留、必填校验、未知道具拒绝、
//! application/x-www-form-urlencoded 与 multipart/form-data 两种编码。
//!
//! 边界（诚实声明）：按 HTML 表单提交语义编码——只有 checked 的 checkbox/radio
//! 才提交，同名多值（多 checkbox/radio、同名 text、select multiple）按控件出现
//! 顺序全部提交；不实现 datalist、不做客户端约束验证（pattern/min/max）。
//! 完整表单抽取属于 PR 06（FORM-09 多表单页选择），本模块提供 form_index。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use webctl_core::ErrorCode;

mod encode;
mod parse;

pub use parse::parse_forms;

#[cfg(test)]
mod tests;

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
