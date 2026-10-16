//! SPA / JS 降级语义（PLAN v0.4 §5.2 PR 09，议题 #38）。
//!
//! v0.4 没有 JS 引擎，也没有 live DOM。「降级」不是失败，而是**诚实标注**：
//! 页面需要客户端渲染时，我们只能返回预渲染 HTML，必须让调用方一眼看出
//! 「这不是渲染后的内容」。本模块给出三件事：
//!
//! 1. detect：判定页面是否依赖 JS，并产出可解释依据（脚本密度、空容器、
//!    noscript 占比、框架指纹）；静态但脚本多的页面**不得**被判为降级（DEGRADE-06）；
//! 2. guard：把判定写进结果信封（code + error + meta.degraded_from 成对）；
//! 3. 强制开关：显式跳过判定，原样返回响应体，但仍标 degraded_from（DEGRADE-07）。
//!
//! # 语义表
//!
//! | 情形 | ok | code | degraded_from | data |
//! |---|---|---|---|---|
//! | 服务端渲染充分（含静态脚本多页） | true | OK | null | 原结果 |
//! | fetch 到需要 JS 的页面 | true | DEGRADED | js-render | 原始 HTML，原样 |
//! | extract 零命中且页面需要 JS（有占位/回退） | true | DEGRADED | js-render | 空结果 + 结论 |
//! | extract 零命中且页面是纯空壳 | true | UNSUPPORTED | js-render | 空结果 + 为何无内容 |
//! | 显式强制开关 | true | DEGRADED | js-render | 原始内容，原样 |
//!
//! 边界与 PR 06 的 EXTRACT-06 严格区分：**零命中本身可以是 OK**；只有当页面
//! 被判定为需要 JS 时，零命中才必须是非 OK 的降级码。静态页面的零命中仍是
//! OK + 空数组 + 可读结论。
//!
//! 禁止编造（DEGRADE-10）：本模块不产生任何页面内容，只读取输入并写诊断；
//! guard 从不修改 data，测试以「data 逐字节不变」固化这一约束。

pub mod detect;
pub mod guard;

pub use detect::{Assessment, DEGRADED_FROM, Diagnosis, Severity, assess, signals};
pub use guard::{DegradeOptions, Scope, guard};
