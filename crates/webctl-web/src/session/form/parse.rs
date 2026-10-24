//! 表单解析（由 `form.rs` 拆分，只移动代码不改行为）。

use super::{Field, FieldKind, Form, FormEnctype, FormMethod, SelectOption};
use crate::session::html::{self, Node};

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
pub(super) fn resolve_positional<T: Clone>(
    list: Option<&[T]>,
    index: usize,
    total: usize,
) -> Option<T> {
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
