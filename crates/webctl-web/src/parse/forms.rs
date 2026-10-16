//! 表单抽取（PARSE-09）。
//!
//! 输出面向 PR 04 的 fill_form：字段用 name 寻址，类型归一化，
//! 默认值/勾选态/必填/禁用/多选都显式给出，select 附完整 option 清单，
//! 表单给出原始 action 与绝对化后的 action_url、method、enctype。
//!
//! 边界：
//! - 无 name 的字段仍然返回（name=None），不静默丢弃；
//! - action 为空时 action_url 取文档 URL；无法绝对化时 action_url=None 而
//!   action 原样保留（不伪造）。
//! - 零表单是合法空结果：ok=true 且 data 为空数组。

use std::time::Instant;

use scraper::ElementRef;
use serde::{Deserialize, Serialize};
use url::Url;
use webctl_core::{Envelope, ErrorCode, Trust};

use super::{elapsed_ms, html};

/// select 的一个选项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormOption {
    /// option 的 value（缺省时退化为文本）。
    pub value: String,
    /// option 的可见文本。
    pub label: String,
    /// 是否默认选中。
    pub selected: bool,
    /// 是否禁用。
    pub disabled: bool,
}

/// 一个可提交字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormField {
    /// name 属性；缺失时为 None。
    pub name: Option<String>,
    /// 归一化类型：input 的 type（默认 text）、select、textarea、button。
    pub field_type: String,
    /// 默认值：input/rextarea 的 value，select 的选中项 value。
    pub default_value: Option<String>,
    /// checkbox/radio 是否默认勾选。
    pub checked: bool,
    /// required 属性。
    pub required: bool,
    /// disabled 属性。
    pub disabled: bool,
    /// readonly 属性。
    pub readonly: bool,
    /// select 是否多选。
    pub multiple: bool,
    /// placeholder 属性。
    pub placeholder: Option<String>,
    /// id 属性。
    pub id: Option<String>,
    /// 关联 label 文本（for=id 优先，其次包裹式 label）。
    pub label: Option<String>,
    /// select 的选项清单；其它类型为空数组。
    pub options: Vec<FormOption>,
}

/// 一个表单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Form {
    /// 文档序序号（0 起）。
    pub index: usize,
    /// id 属性。
    pub id: Option<String>,
    /// name 属性。
    pub name: Option<String>,
    /// 原始 action 属性。
    pub action: Option<String>,
    /// 绝对化后的提交地址。
    pub action_url: Option<String>,
    /// 归一化 method（小写；缺省 get）。
    pub method: String,
    /// enctype（缺省 application/x-www-form-urlencoded）。
    pub enctype: String,
    /// 字段清单（文档序）。
    pub fields: Vec<FormField>,
}

/// 抽取全部表单；零命中是合法空结果。
pub fn extract_forms(html_text: &str, base: Option<&str>) -> Envelope<Vec<Form>> {
    let started = Instant::now();
    let (form_css, field_css, option_css, label_css) = match (
        html::compile_selector("form"),
        html::compile_selector("input, select, textarea, button"),
        html::compile_selector("option"),
        html::compile_selector("label[for]"),
    ) {
        (Ok(a), Ok(b), Ok(c), Ok(d)) => (a, b, c, d),
        (Err(message), ..) | (_, Err(message), ..) => {
            return Envelope::fail(ErrorCode::Parse, message, elapsed_ms(started));
        }
        _ => {
            return Envelope::fail(
                ErrorCode::Parse,
                "invalid built-in selector",
                elapsed_ms(started),
            );
        }
    };
    let document = html::parse_document(html_text);
    let base_url = html::resolve_base(&document, base);
    let labels: Vec<(String, String)> = document
        .select(&label_css)
        .filter_map(|element| {
            let for_id = element.attr("for")?.trim();
            if for_id.is_empty() {
                return None;
            }
            Some((
                for_id.to_owned(),
                html::collapse_ws(&element.text().collect::<String>()),
            ))
        })
        .collect();
    let forms = document
        .select(&form_css)
        .enumerate()
        .map(|(index, form)| build_form(index, form, &base_url, &labels, &field_css, &option_css))
        .collect();
    Envelope::ok(forms, Trust::UntrustedPageContent, elapsed_ms(started))
}

fn build_form(
    index: usize,
    form: ElementRef<'_>,
    base_url: &Option<Url>,
    labels: &[(String, String)],
    field_css: &scraper::Selector,
    option_css: &scraper::Selector,
) -> Form {
    let action = form
        .attr("action")
        .map(str::to_owned)
        .filter(|value| !value.trim().is_empty());
    let action_url = match (action.as_ref(), base_url) {
        (Some(raw), Some(base)) => base.join(raw).ok().map(|url| url.to_string()),
        (Some(raw), None) => Url::parse(raw).ok().map(|url| url.to_string()),
        (None, Some(base)) => Some(base.to_string()),
        (None, None) => None,
    };
    let method = form
        .attr("method")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "get".to_owned());
    let enctype = form
        .attr("enctype")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "application/x-www-form-urlencoded".to_owned());
    let fields = form
        .select(field_css)
        .filter_map(|element| build_field(element, labels, option_css))
        .collect();
    Form {
        index,
        id: form.attr("id").map(str::to_owned),
        name: form.attr("name").map(str::to_owned),
        action,
        action_url,
        method,
        enctype,
        fields,
    }
}

fn build_field(
    element: ElementRef<'_>,
    labels: &[(String, String)],
    option_css: &scraper::Selector,
) -> Option<FormField> {
    let tag = element.value().name();
    let field_type = match tag {
        "textarea" => "textarea".to_owned(),
        "select" => "select".to_owned(),
        "button" => element
            .attr("type")
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "submit".to_owned()),
        "input" => element
            .attr("type")
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "text".to_owned()),
        _ => return None,
    };
    let id = element.attr("id").map(str::to_owned);
    let options: Vec<FormOption> = if tag == "select" {
        element
            .select(option_css)
            .map(|option| {
                let label = html::collapse_ws(&option.text().collect::<String>());
                let value = option
                    .attr("value")
                    .map(str::to_owned)
                    .unwrap_or_else(|| label.clone());
                FormOption {
                    value,
                    label,
                    selected: option.attr("selected").is_some(),
                    disabled: option.attr("disabled").is_some(),
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let default_value = match tag {
        "textarea" => {
            let text = element.text().collect::<String>();
            if text.is_empty() { None } else { Some(text) }
        }
        "select" => options
            .iter()
            .find(|option| option.selected)
            .or_else(|| options.first())
            .map(|option| option.value.clone()),
        _ => element.attr("value").map(str::to_owned),
    };
    let label = id
        .as_ref()
        .and_then(|id| {
            labels
                .iter()
                .find(|(for_id, _)| for_id == id)
                .map(|(_, text)| text.clone())
        })
        .or_else(|| ancestor_label(element));
    Some(FormField {
        name: element.attr("name").map(str::to_owned),
        field_type,
        default_value,
        checked: element.attr("checked").is_some(),
        required: element.attr("required").is_some(),
        disabled: element.attr("disabled").is_some(),
        readonly: element.attr("readonly").is_some(),
        multiple: element.attr("multiple").is_some(),
        placeholder: element.attr("placeholder").map(str::to_owned),
        id,
        label,
        options,
    })
}

fn ancestor_label(element: ElementRef<'_>) -> Option<String> {
    // ego_tree 的 ancestors() 从父节点开始（不含自身）。
    for ancestor in element.ancestors() {
        if let Some(candidate) = ElementRef::wrap(ancestor)
            && candidate.value().name() == "label"
        {
            return Some(html::collapse_ws(&candidate.text().collect::<String>()));
        }
    }
    None
}
