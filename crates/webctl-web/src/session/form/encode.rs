//! 表单编码与提交体构造（由 `form.rs` 拆分，只移动代码不改行为）。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use super::parse::resolve_positional;
use super::{EncodedBody, Field, FieldKind, Form, FormEnctype, FormError, FormFill};

impl Form {
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
