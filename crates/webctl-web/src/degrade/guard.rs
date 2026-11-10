//! 信封守卫层：把「需要 JS」的判定写进结果信封（DEGRADE-07/08/09）。
//!
//! 守卫只改写信封的 code / error / meta.degraded_from，**原样保留 data**：
//! 不渲染、不补全、不编造任何内容（DEGRADE-10）。这样 fetch 仍能拿到原始 HTML，
//! extract 仍能拿到诚实的空结果与可读结论。
//!
//! 码的边界（A6：空结果、失败、不支持、超限互不混淆）：
//! - Scope::Fetch：原始内容仍在 data 里 → DEGRADED（DEGRADE-01/04/05/07）。
//! - Scope::EmptyExtract：结构化结果为空 → 有回退/占位内容时 DEGRADED，
//!   连占位都没有时 UNSUPPORTED（DEGRADE-08/09）。
//!
//! 两者都不是 OK：绝不返回「成功但 0 条」。

use webctl_core::{Actionable, Envelope, ErrorCode, FallbackHint};

use super::detect::{self, Assessment, Diagnosis, Severity};
use super::fallback;

/// 守卫作用域：决定需要 JS 时空结果用哪个降级码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// 取回入口：原始内容仍在 data 里，标 DEGRADED。
    Fetch,
    /// 抽取入口且结果为空：标 DEGRADED 或 UNSUPPORTED。
    EmptyExtract,
}

/// 守卫选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DegradeOptions {
    /// DEGRADE-07：跳过启发式判定，直接把结果按「需要 JS」处理，内容原样返回。
    pub force: bool,
}

/// 对已成功的信封做判定并写降级；未命中需要 JS 时原样返回。
///
/// - 失败信封不处理（保持原码）；
/// - 已有 degraded_from 的信封不覆盖（例如响应体截断已经标了 LIMIT）；
/// - force 时跳过判定，直接按需要 JS 降级（DEGRADE-07）。
pub fn guard<T>(
    html: &str,
    envelope: Envelope<T>,
    options: &DegradeOptions,
    scope: Scope,
) -> Envelope<T> {
    if !envelope.ok || envelope.meta.degraded_from.is_some() {
        return envelope;
    }
    let (severity, diagnosis) = if options.force {
        (Severity::Total, Diagnosis::forced())
    } else {
        match detect::assess(html) {
            Assessment::Rendered => return envelope,
            Assessment::NeedsJs {
                severity,
                diagnosis,
            } => (severity, diagnosis),
        }
    };
    let code = match scope {
        Scope::Fetch => ErrorCode::Degraded,
        Scope::EmptyExtract => match severity {
            Severity::Partial => ErrorCode::Degraded,
            Severity::Total => ErrorCode::Unsupported,
        },
    };
    // #224：fetch/extract 不自动抽取内嵌数据，所以 embedded_data_tried 恒为 false。
    let fallback_hint = fallback::hint(html, false);
    apply(envelope, code, &diagnosis, fallback_hint)
}

/// 只写 code / error / degraded_from，data 一个字节都不动。
fn apply<T>(
    mut envelope: Envelope<T>,
    code: ErrorCode,
    diagnosis: &Diagnosis,
    fallback_hint: Option<FallbackHint>,
) -> Envelope<T> {
    envelope.code = code;
    envelope.meta.degraded_from = Some(diagnosis.degraded_from.clone());
    envelope.error = diagnosis.reason.clone();
    // #115：把"为什么 + 还能试什么"挂到 meta.actionable，让调用方不必猜。
    let mut next_steps = diagnosis.next_steps.clone();
    // #225：宿主声明了外部回退工具且本页适合外部浏览器时，才追加一条 `use <tool>`。
    // 未声明时 next_steps 保持原样（不含任何外部工具名）。
    fallback::push_declared_next_step(fallback_hint.as_ref(), &mut next_steps);
    envelope.with_actionable(Actionable {
        reason: diagnosis.reason_code.clone(),
        detected: diagnosis.evidence.clone(),
        next_steps,
        fallback_hint,
    })
}
