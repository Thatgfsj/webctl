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
///
/// **按浏览器口径**处理 RAWTEXT（`title` / `style` / `textarea`）：它们里面的 `<script>` 不是脚本，
/// 不能执行——否则我们会跑一段浏览器不会跑的代码（复审实测过这种注入形态）。
fn extract_scripts(html: &str) -> Vec<ScriptBlock> {
    const RAWTEXT: [&str; 3] = ["title", "style", "textarea"];
    let lowered = html.to_ascii_lowercase();
    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    while let Some(lt_rel) = lowered[cursor..].find('<') {
        let lt = cursor + lt_rel;
        let rest = &lowered[lt + 1..];
        if rest.starts_with('/') || rest.starts_with('!') || rest.starts_with('?') {
            match lowered[lt..].find('>') {
                Some(gt) => cursor = lt + gt + 1,
                None => break,
            }
            continue;
        }
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        let Some(gt_rel) = lowered[lt..].find('>') else {
            break;
        };
        let open_end = lt + gt_rel;
        let attrs = &lowered[lt + 1 + name.len()..open_end];
        if name == "script" {
            let body_start = open_end + 1;
            let Some(close_rel) = lowered[body_start..].find("</script") else {
                break;
            };
            let body_end = body_start + close_rel;
            let body = html[body_start..body_end].to_owned();
            blocks.push(ScriptBlock {
                runnable: is_runnable(attrs) && !body.trim().is_empty(),
                body,
            });
            let Some(close_gt) = lowered[body_end..].find('>') else {
                break;
            };
            cursor = body_end + close_gt + 1;
            continue;
        }
        if RAWTEXT.contains(&name.as_str()) {
            // 跳到匹配的结束标签之后：里面的 `<script>` 不是脚本。
            let needle = format!("</{name}");
            match lowered[open_end + 1..].find(&needle) {
                Some(rel) => match lowered[open_end + 1 + rel..].find('>') {
                    Some(gt) => cursor = open_end + 1 + rel + gt + 1,
                    None => break,
                },
                None => break,
            }
            continue;
        }
        cursor = open_end + 1;
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

    /// 去掉所有 RAWTEXT 元素的正文（`script`/`style`/`textarea`/`title`）。
    ///
    /// 渲染结果**忠实保留**这些元素的原文（与浏览器一致），所以判定"页面内容"
    /// 必须先剥掉它们——否则脚本/样式里的模板字符串会被算进来（本文件踩过两次）。
    fn strip_rawtext(html: &str) -> String {
        let mut out = html.to_owned();
        for tag in ["script", "style", "textarea", "title"] {
            let mut cursor = 0usize;
            loop {
                let lowered = out.to_ascii_lowercase();
                let Some(start_rel) = lowered[cursor..].find(&format!("<{tag}")) else {
                    break;
                };
                let start = cursor + start_rel;
                let Some(open_gt) = lowered[start..].find('>') else {
                    break;
                };
                let body_start = start + open_gt + 1;
                let Some(close_rel) = lowered[body_start..].find(&format!("</{tag}")) else {
                    break;
                };
                let body_end = body_start + close_rel;
                out.replace_range(body_start..body_end, "");
                cursor = body_start;
            }
        }
        out
    }

    /// #128：仓库自己的 JS 空壳样本经渲染后应出现 10 条内容（原本只在脚本里）。
    #[test]
    fn renders_document_write_page() {
        let outcome = render(JS_SHELL, &JsLimits::default()).expect("应能渲染");
        assert_eq!(outcome.executed_scripts, 1, "样本只有一段内联脚本");
        // 两条断言作用不同（#133 复审用对照实验校正过）：
        //   · `contains("class='quote'")`：**判别 RAWTEXT 回归**（脚本正文是否按原文保留）；
        //   · `overall == 10`：防"幽灵节点泄漏到输出"（例如脚本正文被当 DOM 且序列化没有按
        //     原文处理 script 子节点时，输出里会多出元素——那才会数成 11）。
        let overall = outcome.html.matches("class=\"quote\"").count();
        // 这条防的是"幽灵节点泄漏到输出"；它**单独不能**判别 RAWTEXT 回归——
        // 若脚本正文被当 DOM 解析，序列化对 script 子节点输出 [object Object]，计数仍是 10。
        assert_eq!(overall, 10, "完整输出里应正好 10 条（含脚本正文时）");
        assert!(
            outcome.html.contains("class='quote'"),
            "脚本正文应按原文保留（RAWTEXT），不被解析成 DOM"
        );
        let content = strip_rawtext(&outcome.html);
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

    /// **保真**：`<title>`/`<style>`/`<textarea>` 里的 `<script>` 不是脚本（浏览器不执行）。
    #[test]
    fn script_inside_rawtext_is_not_executed() {
        let html = concat!(
            "<html><head><title>x<script>document.write('<i id=\"INJECTED\"></i>')</script>y</title></head>",
            "<body>ok</body></html>"
        );
        let outcome = render(html, &JsLimits::default()).expect("应能渲染");
        assert_eq!(
            outcome.scripts_seen, 0,
            "title 里的 script 不算脚本：{}",
            outcome.html
        );
        assert_eq!(outcome.executed_scripts, 0);
        // 源码里本来就有 INJECTED 字面量（title 原文），所以剥掉 RAWTEXT 内容后再查注入痕迹。
        let content = strip_rawtext(&outcome.html);
        assert!(
            !content.contains("INJECTED"),
            "不得执行 title 里的脚本：{content}"
        );
    }

    /// **保真**：RAWTEXT 元素的内容按原文序列化（不做 HTML 转义），与浏览器 outerHTML 一致。
    #[test]
    fn rawtext_serializes_verbatim() {
        let html = "<html><head><style>a{content:\"<b>x</b>\"}</style></head><body>t</body></html>";
        let outcome = render(html, &JsLimits::default()).expect("应能渲染");
        assert!(
            outcome.html.contains("a{content:\"<b>x</b>\"}"),
            "style 内容应原样输出：{}",
            outcome.html
        );
    }

    /// **安全**：页面脚本**不能**改写或删除序列化函数（否则可伪造渲染结果 / 让渲染失败）。
    ///
    /// 复审观察：初版把 `__webctl_serialize` 作为可写全局暴露。现改为 `writable: false,
    /// configurable: false, enumerable: false`——只读调用允许（无副作用），改写与删除都无效。
    #[test]
    fn page_cannot_overwrite_serializer() {
        let html = concat!(
            "<html><body><div id=\"x\">真实内容</div><script>",
            "globalThis.__webctl_serialize = function () { return 'PWNED' };",
            "try { delete globalThis.__webctl_serialize; } catch (e) {}",
            "</script></body></html>"
        );
        let outcome = render(html, &JsLimits::default()).expect("应能渲染");
        // 判定"页面内容"前先剥掉脚本正文（RAWTEXT 会忠实保留脚本源码，里面就有 PWNED 字面量）。
        let content = strip_rawtext(&outcome.html);
        assert!(
            content.contains("真实内容"),
            "页面不得伪造渲染结果：{content}"
        );
        assert!(
            !content.contains("PWNED"),
            "被改写后的序列化函数不得生效：{content}"
        );
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
