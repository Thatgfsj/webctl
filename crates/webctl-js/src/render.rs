//! 最小 DOM 渲染（ADR-0005 第 4 条 / 议题 #128 第 1 半）。
//!
//! 流程：从 HTML 里取出**内联**脚本 → 在沙箱里先装最小 DOM 垫片（纯 JS）→ 依次求值脚本 →
//! 序列化出"渲染后的 HTML"，交给既有 Rust 解析/抽取。
//!
//! **安全**：垫片是 JS 字符串，不注册任何 Rust 宿主函数；渲染后的能力面与裸沙箱完全相同
//! （无网络、无文件、无定时器）。有测试用脚本把 `typeof fetch` 写进 DOM 来证明这一点。

use std::time::{Duration, Instant};

use boa_engine::Source;

use crate::{JsFailure, JsFailureKind, JsLimits, check_budget, sandbox};

/// 最小 DOM 垫片源码（占位符 `__WEBCTL_INITIAL_HTML__` 会被替换成初始 HTML 的 JS 字符串字面量）。
const DOM_SHIM: &str = include_str!("dom_shim.js");

/// 渲染结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderOutcome {
    /// 渲染后的 HTML（`document.documentElement` 的序列化）。
    pub html: String,
    /// 实际执行的脚本段数。
    pub executed_scripts: usize,
    /// 页面的内联脚本总段数（含因 `src` / `type=module` 被跳过的）。
    pub scripts_seen: usize,
    /// 耗时（诊断用）。
    pub elapsed: Duration,
}

/// 一段内联脚本。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScriptBlock {
    /// 脚本正文。
    body: String,
    /// 是否可执行（`src` 外链与 `type=module` 不执行）。
    runnable: bool,
}

/// 渲染：执行页面内联脚本，返回渲染后的 HTML。
pub fn render(html: &str, limits: &JsLimits) -> Result<RenderOutcome, JsFailure> {
    let blocks = extract_scripts(html);
    let runnable: Vec<String> = blocks
        .iter()
        .filter(|block| block.runnable)
        .map(|block| block.body.clone())
        .collect();
    check_budget(&runnable, limits)?;

    let started = Instant::now();
    let mut context = sandbox(limits);
    let bootstrap = DOM_SHIM.replace("__WEBCTL_INITIAL_HTML__", &js_string_literal(html));
    eval(&mut context, &bootstrap)?;
    for script in &runnable {
        eval(&mut context, script)?;
    }
    let serialized = eval(&mut context, "globalThis.__webctl_serialize()")?;
    Ok(RenderOutcome {
        html: serialized,
        executed_scripts: runnable.len(),
        scripts_seen: blocks.len(),
        elapsed: started.elapsed(),
    })
}

/// 求值并按 ECMAScript `String(value)` 取结果。
fn eval(context: &mut boa_engine::Context, script: &str) -> Result<String, JsFailure> {
    let value = context
        .eval(Source::from_bytes(script.as_bytes()))
        .map_err(|error| classify(error.to_string()))?;
    value
        .to_string(context)
        .map(|text| text.to_std_string_escaped())
        .map_err(|error| classify(error.to_string()))
}

fn classify(message: String) -> JsFailure {
    let lowered = message.to_ascii_lowercase();
    let kind = if [
        "loop iteration",
        "recursion",
        "recursive",
        "stack size",
        "maximum call stack",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
    {
        JsFailureKind::Limit
    } else if lowered.contains("syntaxerror") || lowered.contains("unexpected token") {
        JsFailureKind::Syntax
    } else {
        JsFailureKind::Runtime
    };
    JsFailure::new(kind, message)
}

/// 把 HTML 里的 `<script>` 逐段取出来（大小写不敏感；只读内联正文）。
fn extract_scripts(html: &str) -> Vec<ScriptBlock> {
    let lowered = html.to_ascii_lowercase();
    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel) = lowered[cursor..].find("<script") {
        let start = cursor + rel;
        let Some(gt_rel) = lowered[start..].find('>') else {
            break;
        };
        let open_end = start + gt_rel;
        let attrs = &lowered[start + "<script".len()..open_end];
        let Some(close_rel) = lowered[open_end + 1..].find("</script") else {
            break;
        };
        let body_start = open_end + 1;
        let body_end = body_start + close_rel;
        let body = html[body_start..body_end].to_owned();
        blocks.push(ScriptBlock {
            runnable: is_runnable(attrs) && !body.trim().is_empty(),
            body,
        });
        cursor = body_end;
    }
    blocks
}

/// 只有内联、非模块脚本才执行：外链（`src`）会引入网络依赖，`module` 需要模块加载器（已 fail-closed）。
fn is_runnable(attrs: &str) -> bool {
    if attrs.contains("src") {
        return false;
    }
    match attribute(attrs, "type") {
        None => true,
        Some(value) => {
            let lowered = value
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_ascii_lowercase();
            matches!(
                lowered.as_str(),
                "" | "text/javascript" | "application/javascript" | "text/ecmascript"
            )
        }
    }
}

fn attribute(attrs: &str, name: &str) -> Option<String> {
    let at = attrs.find(name)?;
    let rest = attrs[at + name.len()..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let end = rest[1..].find(quote)? + 1;
        Some(rest[1..end].to_owned())
    } else {
        Some(
            rest.split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned(),
        )
    }
}

/// 生成 JS 字符串字面量（含控制字符转义），用于把 HTML 安全地喂给垫片。
fn js_string_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const JS_SHELL: &str =
        include_str!("../../webctl-web/tests/fixtures/degrade/js_data_script_shell.html");

    /// 去掉 `<script>…</script>` 正文：渲染结果**忠实保留**原始脚本元素，
    /// 计数"页面内容"时必须先把脚本正文剥掉（否则脚本里的模板字符串会被算进去）。
    fn strip_scripts(html: &str) -> String {
        let lowered = html.to_ascii_lowercase();
        let mut out = String::new();
        let mut cursor = 0usize;
        while let Some(rel) = lowered[cursor..].find("<script") {
            let start = cursor + rel;
            out.push_str(&html[cursor..start]);
            match lowered[start..].find("</script") {
                Some(end_rel) => {
                    let after = start + end_rel;
                    match lowered[after..].find('>') {
                        Some(gt) => cursor = after + gt + 1,
                        None => break,
                    }
                }
                None => break,
            }
        }
        out.push_str(&html[cursor..]);
        out
    }

    /// #128：仓库自己的 JS 空壳样本经渲染后应出现 10 条内容（原本只在脚本里）。
    #[test]
    fn renders_document_write_page() {
        let outcome = render(JS_SHELL, &JsLimits::default()).expect("应能渲染");
        assert_eq!(outcome.executed_scripts, 1, "样本只有一段内联脚本");
        let content = strip_scripts(&outcome.html);
        let quotes = content.matches("class=\"quote\"").count();
        assert_eq!(quotes, 10, "渲染后应有 10 条内容");
        assert!(content.contains("Quote number 10"), "第 10 条应进 DOM");
        assert!(
            outcome.html.contains("<script"),
            "渲染结果应忠实保留原脚本元素"
        );
    }

    /// 静态 DOM 可被查询与修改（querySelector + textContent）。
    #[test]
    fn renders_against_existing_dom() {
        let html = "<html><body><div id=\"app\">旧</div><script>document.querySelector('#app').textContent = '新';</script></body></html>";
        let outcome = render(html, &JsLimits::default()).expect("应能渲染");
        assert!(
            outcome.html.contains(">新<"),
            "应看到修改后的文本：{}",
            outcome.html
        );
    }

    /// 外链脚本与 module 脚本跳过；只有内联可执行。
    #[test]
    fn skips_external_and_module_scripts() {
        let html = concat!(
            "<html><body>",
            "<script src=\"/app.js\"></script>",
            "<script type=\"module\">import './x.js'</script>",
            "<script>document.title = 'ok'</script>",
            "</body></html>"
        );
        let outcome = render(html, &JsLimits::default()).expect("应能渲染");
        assert_eq!(outcome.scripts_seen, 3, "应看到 3 段");
        assert_eq!(outcome.executed_scripts, 1, "只执行内联那段");
        assert!(
            outcome.html.contains("<title>ok</title>"),
            "{}",
            outcome.html
        );
    }

    /// **安全**：装好 DOM 之后能力面不变——脚本自己把 `typeof fetch` 写进 DOM 来证明。
    #[test]
    fn dom_shim_adds_no_host_capabilities() {
        let html = concat!(
            "<html><body><div id=\"probe\"></div><script>",
            "document.querySelector('#probe').textContent = [typeof fetch, typeof require, typeof process, typeof setTimeout, typeof XMLHttpRequest].join(',');",
            "</script></body></html>"
        );
        let outcome = render(html, &JsLimits::default()).expect("应能渲染");
        assert!(
            outcome
                .html
                .contains("undefined,undefined,undefined,undefined,undefined"),
            "装 DOM 后不得引入宿主能力：{}",
            outcome.html
        );
    }

    /// 脚本预算仍然生效（超段数直接拒绝）。
    #[test]
    fn render_respects_script_budget() {
        let limits = JsLimits {
            max_scripts: 1,
            ..JsLimits::default()
        };
        let html = "<html><body><script>1</script><script>2</script></body></html>";
        let failure = render(html, &limits).expect_err("应被拒");
        assert_eq!(failure.kind, JsFailureKind::Rejected, "{failure}");
    }

    /// **安全**：页面脚本同样受引擎限值约束（死循环被中断，不会挂住渲染）。
    #[test]
    fn render_limits_apply_to_page_scripts() {
        let limits = JsLimits {
            loop_iteration_limit: 100_000,
            ..JsLimits::default()
        };
        let html = "<html><body><script>while (true) {}</script></body></html>";
        let failure = render(html, &limits).expect_err("死循环必须被中断");
        assert_eq!(failure.kind, JsFailureKind::Limit, "{failure}");
    }

    /// 渲染是确定性的（同一页面两次结果一致）。
    #[test]
    fn render_is_deterministic() {
        let first = render(JS_SHELL, &JsLimits::default()).expect("第一次");
        let second = render(JS_SHELL, &JsLimits::default()).expect("第二次");
        assert_eq!(first.html, second.html);
    }
}
