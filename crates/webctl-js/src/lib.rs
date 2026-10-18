//! 受限内置 JS 引擎的**沙箱核心**（[ADR-0005](../../docs/adr/0005-embedded-js-engine.md) / 议题 #127）。
//!
//! 本 crate 是**唯一**被允许依赖 `boa_engine` 的地方——依赖边界守卫会机械校验这一点。
//!
//! ## 承诺什么
//!
//! - **零宿主能力**：不注入 `fetch` / `XMLHttpRequest` / `require` / `process` / `setTimeout` 等；
//!   沙箱里只有 ECMAScript 标准内建。有测试逐项断言这些全局**不存在**。
//! - **可中断**：死循环与深递归在限值处被中断，返回**显式错误**——绝不假装成功。
//! - **确定性（有边界）**：不注入**宿主**时间/随机源；ECMAScript 自带的 `Date` / `Math.random` 仍在，
//!   依赖它们的脚本两次求值可能不同。对不依赖时间/随机的脚本，结果一致（有测试）。
//!
//! ## 不承诺什么（诚实边界，不许宣传成"有墙钟上限"）
//!
//! `boa_engine` 0.20 **没有抢占式中断**（无墙钟/指令数中断 API），只有 loop / recursion / stack 限值。
//! 因此"任意脚本都在 X 毫秒内结束"**不是**本 crate 的承诺：直线型长脚本只受"脚本字节上限 + 引擎吞吐"约束。
//! CPU/内存的硬隔离由进程级手段（如 `ulimit`）承担，不由本 crate 冒充。
//!
//! **具体一点**：`boa_engine` 0.20 **没有堆内存/GC 上限 API**，因此本 crate **不限制堆分配**——
//! 实测 `new Uint8Array(256 * 1024 * 1024)` 会成功（约 0.9 s）。真要卡内存，请配合进程级限制
//! （`ulimit -v` 等），或等 [#130](https://github.com/Thatgfsj/webctl/issues/130) 的宿主侧方案。

use std::rc::Rc;
use std::time::{Duration, Instant};

pub mod render;

// 方便调用方直接用 `webctl_js::render(...)`（函数在值命名空间，与模块名不冲突）。
pub use render::{RenderOutcome, render};

use boa_engine::module::IdleModuleLoader;
use boa_engine::{Context, Source};

/// 沙箱限值（全部可下调；默认值按"取出一个页面的文本"这个目标设定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsLimits {
    /// 最多接受多少段脚本。
    pub max_scripts: usize,
    /// 单段脚本字节上限。
    pub max_script_bytes: usize,
    /// 所有脚本合计字节上限。
    pub max_total_bytes: usize,
    /// 引擎循环迭代上限（防死循环）。
    pub loop_iteration_limit: u64,
    /// 递归深度上限（防深递归）。
    pub recursion_limit: usize,
    /// 引擎栈大小上限（防栈内存爆炸）。
    pub stack_size_limit: usize,
}

impl Default for JsLimits {
    fn default() -> Self {
        Self {
            max_scripts: 32,
            max_script_bytes: 256 * 1024,
            max_total_bytes: 512 * 1024,
            loop_iteration_limit: 1_000_000,
            recursion_limit: 256,
            stack_size_limit: 1024,
        }
    }
}

/// 失败类别（对调用方是封闭集）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsFailureKind {
    /// 输入被拒（超尺寸、脚本条数超限等），**没有执行**。
    Rejected,
    /// 引擎限值触发（死循环 / 深递归 / 栈上限）。
    Limit,
    /// 语法错误。
    Syntax,
    /// 运行期异常。
    Runtime,
}

/// 一次失败的详情。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsFailure {
    /// 类别。
    pub kind: JsFailureKind,
    /// 人类可读说明（不得为空）。
    pub message: String,
}

impl JsFailure {
    pub(crate) fn new(kind: JsFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for JsFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for JsFailure {}

/// 一次求值的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsOutcome {
    /// 最后一段脚本的求值结果（字符串形式；`undefined` 时就是 `"undefined"`）。
    pub value: String,
    /// 实际执行的脚本段数。
    pub executed_scripts: usize,
    /// 求值耗时（仅诊断用，不是指标）。
    pub elapsed: Duration,
}

/// 求值一段脚本（零宿主能力）。
pub fn run(script: &str, limits: &JsLimits) -> Result<JsOutcome, JsFailure> {
    run_scripts(std::slice::from_ref(&script.to_owned()), limits)
}

/// 在**同一个**上下文里依次求值多段脚本（模拟一个页面的多段内联脚本共享全局状态）。
pub fn run_scripts(scripts: &[String], limits: &JsLimits) -> Result<JsOutcome, JsFailure> {
    check_budget(scripts, limits)?;
    let mut context = sandbox(limits);
    let started = Instant::now();
    let mut value = "undefined".to_owned();
    for script in scripts {
        let source = Source::from_bytes(script.as_bytes());
        match context.eval(source) {
            // 用 ECMAScript 的 `String(value)` 语义，而不是 `display()`：
            // display() 会给字符串结果加引号并转义，调用方拿到的是"源码形式"而不是"值"。
            Ok(result) => {
                value = result
                    .to_string(&mut context)
                    .map(|text| text.to_std_string_escaped())
                    .map_err(|error| classify(&error.to_string()))?;
            }
            Err(error) => return Err(classify(&error.to_string())),
        }
    }
    Ok(JsOutcome {
        value,
        executed_scripts: scripts.len(),
        elapsed: started.elapsed(),
    })
}

/// 建一个**只有 ECMAScript 内建**的上下文，并装上全部限值。
///
/// **安全关键**：这里**不能**用 `Context::default()`。boa 0.20 在未显式指定模块加载器时，
/// 会装上以**进程 CWD 为根**的 `SimpleModuleLoader`（`boa_engine` 源码里 `SimpleModuleLoader::new(Path::new("."))`），
/// 于是页面脚本里的 `import('相对路径.js')` 会真的打开并读取宿主文件——
/// 这一点已由独立安全审阅用 FIFO 阻塞（恰好 2.000 s）实证，属**真实逃逸**。
/// 显式装 [`IdleModuleLoader`] 后，模块解析一律 fail-closed。
///
/// 另一层防御：本 crate **从不**调用 `run_jobs()`，因此 promise 任务永远不会被排空。
pub(crate) fn sandbox(limits: &JsLimits) -> Context {
    let mut context = Context::builder()
        .module_loader(Rc::new(IdleModuleLoader))
        .build()
        .expect("沙箱上下文应当能建起来");
    let runtime = context.runtime_limits_mut();
    runtime.set_loop_iteration_limit(limits.loop_iteration_limit);
    runtime.set_recursion_limit(limits.recursion_limit);
    runtime.set_stack_size_limit(limits.stack_size_limit);
    context
}

/// 输入预算检查：超限**直接拒绝**，不进入引擎。
pub(crate) fn check_budget(scripts: &[String], limits: &JsLimits) -> Result<(), JsFailure> {
    if scripts.len() > limits.max_scripts {
        return Err(JsFailure::new(
            JsFailureKind::Rejected,
            format!("脚本段数 {} 超过上限 {}", scripts.len(), limits.max_scripts),
        ));
    }
    let mut total = 0usize;
    for script in scripts {
        let bytes = script.len();
        if bytes > limits.max_script_bytes {
            return Err(JsFailure::new(
                JsFailureKind::Rejected,
                format!("单段脚本 {bytes} 字节超过上限 {}", limits.max_script_bytes),
            ));
        }
        total += bytes;
    }
    if total > limits.max_total_bytes {
        return Err(JsFailure::new(
            JsFailureKind::Rejected,
            format!("脚本合计 {total} 字节超过上限 {}", limits.max_total_bytes),
        ));
    }
    Ok(())
}

/// 把引擎错误归类。思路：限值类错误必须能被识别出来（否则会被误报成普通运行期异常）。
fn classify(message: &str) -> JsFailure {
    let lowered = message.to_ascii_lowercase();
    let limit_hit = [
        "loop iteration",
        "recursion",
        "recursive",
        "stack size",
        "maximum call stack",
        "limit exceeded",
    ]
    .iter()
    .any(|needle| lowered.contains(needle));
    let syntax = lowered.contains("syntaxerror") || lowered.contains("unexpected token");
    let kind = if limit_hit {
        JsFailureKind::Limit
    } else if syntax {
        JsFailureKind::Syntax
    } else {
        JsFailureKind::Runtime
    };
    JsFailure::new(kind, message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_scripts_in_one_shared_context() {
        let scripts = vec!["globalThis.acc = 2;".to_owned(), "acc * 21".to_owned()];
        let outcome = run_scripts(&scripts, &JsLimits::default()).expect("应能求值");
        assert_eq!(outcome.value, "42");
        assert_eq!(outcome.executed_scripts, 2);
    }

    #[test]
    fn infinite_loop_is_interrupted() {
        let limits = JsLimits {
            loop_iteration_limit: 100_000,
            ..JsLimits::default()
        };
        let failure = run("while (true) {}", &limits).expect_err("死循环必须被中断");
        assert_eq!(failure.kind, JsFailureKind::Limit, "{failure}");
        assert!(!failure.message.is_empty());
    }

    #[test]
    fn deep_recursion_is_interrupted() {
        let limits = JsLimits {
            recursion_limit: 64,
            ..JsLimits::default()
        };
        let failure =
            run("function f(n){return f(n+1)}; f(0)", &limits).expect_err("深递归必须被中断");
        assert_eq!(failure.kind, JsFailureKind::Limit, "{failure}");
    }

    #[test]
    fn oversized_input_is_rejected_without_executing() {
        let limits = JsLimits {
            max_script_bytes: 16,
            ..JsLimits::default()
        };
        let failure = run("globalThis.sideEffect = 1;", &limits).expect_err("应被拒");
        assert_eq!(failure.kind, JsFailureKind::Rejected, "{failure}");
    }

    #[test]
    fn too_many_scripts_are_rejected() {
        let limits = JsLimits {
            max_scripts: 1,
            ..JsLimits::default()
        };
        let scripts = vec!["1".to_owned(), "2".to_owned()];
        let failure = run_scripts(&scripts, &limits).expect_err("应被拒");
        assert_eq!(failure.kind, JsFailureKind::Rejected, "{failure}");
    }

    /// ADR-0005 第 3 条：引擎内**不暴露**网络/文件/子进程/定时器等宿主能力。
    #[test]
    fn host_capabilities_are_absent() {
        let absent = [
            "fetch",
            "XMLHttpRequest",
            "require",
            "process",
            "Deno",
            "WebSocket",
            "setTimeout",
            "setInterval",
            "globalThis.Buffer",
        ];
        let script = absent
            .iter()
            .map(|name| format!("typeof {name}"))
            .collect::<Vec<_>>()
            .join(", ");
        let outcome =
            run(&format!("JSON.stringify([{script}])"), &JsLimits::default()).expect("应能求值");
        assert_eq!(
            outcome.value,
            r#"["undefined","undefined","undefined","undefined","undefined","undefined","undefined","undefined","undefined"]"#,
            "沙箱不得暴露宿主能力，实测：{}",
            outcome.value
        );
    }

    #[test]
    fn evaluation_is_deterministic() {
        let script = "JSON.stringify({a:1,b:[2,3]})";
        let first = run(script, &JsLimits::default()).expect("第一次");
        let second = run(script, &JsLimits::default()).expect("第二次");
        assert_eq!(first.value, second.value);
    }

    /// **安全回归（相对说明符）**：最危险的形态是"CWD 相对路径"（旧默认加载器把 CWD 当 root）。
    ///
    /// 这条用**裸说明符**断言 fail-closed：旧加载器下该说明符会走到"打开文件"分支，
    /// 现在必须是"模块解析被禁用"。绝对路径用例覆盖"存在的文件不被加载"，本条覆盖"相对说明符"。
    #[test]
    fn relative_dynamic_import_is_also_disabled() {
        let mut context = sandbox(&JsLimits::default());
        let script = "import('definitely-not-a-module.js').then(() => { globalThis.__REL = 'resolved' }, e => { globalThis.__REL = String(e) })";
        context
            .eval(Source::from_bytes(script.as_bytes()))
            .expect("import 表达式本身不应抛错");
        context.run_jobs();
        let outcome = context
            .eval(Source::from_bytes(b"String(globalThis.__REL)"))
            .expect("读结果")
            .to_string(&mut context)
            .expect("转字符串")
            .to_std_string_escaped();
        assert!(
            outcome.contains("module resolution is disabled"),
            "相对说明符同样必须 fail-closed（旧加载器会去 open 文件），实测：{outcome}"
        );
    }

    /// **安全回归**：动态 `import()` 必须 fail-closed，且**不得**读取宿主文件。
    ///
    /// 起因：第一版用 `Context::default()`，boa 会装上以 CWD 为根的模块加载器——
    /// 独立安全审阅用 FIFO 阻塞 2.000 s 证明 `import('相对路径.js')` 真的 open/read 了宿主文件。
    /// 这条用例用**确定性**方式验证修复：模块文件**存在**且会设置 `globalThis.__LOADED`，
    /// 排空任务队列后必须既拿到"解析被禁用"，又看到那个全局**仍未定义**。
    #[test]
    fn dynamic_import_is_disabled_fail_closed() {
        let dir = std::env::temp_dir().join(format!("webctl-js-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let target = dir.join("hostile_module.js");
        std::fs::write(&target, "globalThis.__LOADED = true; export const x = 1;")
            .expect("写临时模块");

        let mut context = sandbox(&JsLimits::default());
        let script = format!(
            "import({:?}).then(() => {{ globalThis.__OUTCOME = 'resolved' }}, e => {{ globalThis.__OUTCOME = String(e) }})",
            target.to_string_lossy()
        );
        context
            .eval(Source::from_bytes(script.as_bytes()))
            .expect("import 表达式本身不应抛错");
        context.run_jobs();
        let outcome = context
            .eval(Source::from_bytes(
                b"String(globalThis.__OUTCOME) + '|' + typeof globalThis.__LOADED",
            ))
            .expect("读结果")
            .to_string(&mut context)
            .expect("转字符串")
            .to_std_string_escaped();
        assert!(
            outcome.contains("module resolution is disabled"),
            "动态 import 必须 fail-closed，实测：{outcome}"
        );
        assert!(
            outcome.ends_with("|undefined"),
            "宿主模块文件不得被加载/执行，实测：{outcome}"
        );
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn syntax_error_is_classified() {
        let failure = run("function (", &JsLimits::default()).expect_err("语法错误");
        assert_eq!(failure.kind, JsFailureKind::Syntax, "{failure}");
    }
}
