# ADR-0009：自适应定位只返回显式候选，不自动替换

| 项 | 值 |
|---|---|
| 状态 | **草案（Proposed，2026-11-23，未采纳）** |
| 关联 | [#195](https://github.com/Thatgfsj/webctl/issues/195)；[spike 报告](../spikes/adaptive-selectors.md) |
| 决策者 | 待定 |

## 背景

`extract` 依赖固定 CSS 选择器。站点改版后选择器零命中时，当前实现如实返回
`ok=true` + 空结果 + 可读 conclusion（EXTRACT-06），不做任何猜测。这是正确的默认行为，
但对"长期监控同一页面"的场景缺少恢复手段。

Spike #195 验证了：在文本与祖先结构基本保留的受控改版上，可以用元素指纹
（tag / 稳定 id-class / 文本 / 邻近文本 / 层级路径）按相似度找回候选；重复 DOM
会天然歧义，内容与结构大改会正确失败。报告见 [docs/spikes/adaptive-selectors.md](../spikes/adaptive-selectors.md)。

本 ADR 记录"如果实现，边界必须是什么"，**不是**已经采纳的实现承诺。

## 决策（建议）

1. **自适应定位是显式 opt-in，不是默认行为。**
   默认 `extract` 路径不读取指纹、不跑相似度、不改变零命中语义。调用方必须显式提供
   上一版指纹或开启跟踪参数，才可能得到相似度候选。
2. **相似度结果永远是候选，不是命中。**
   输出必须同时带 `matched_by=similarity`、`confidence`、`requires_confirmation=true`
   与逐条 `evidence`；不得写入 `matches` 冒充精确命中，不得把零命中包装成成功定位。
3. **精确选择器优先。**
   只要当前文档仍有精确命中，就返回 `matched_by=selector`，不跑相似度。
4. **歧义与指纹不足必须拒绝。**
   前两名候选分差低于阈值 → `ambiguous`；可用特征少于 2 个 → `insufficient_fingerprint`；
   最高分低于阈值 → `not_found`。三者都不自动选一个。
5. **预算有硬上限。**
   候选扫描、候选数、墙钟与文本长度都有上限；截断必须返回 `LIMIT` 与非空
   `meta.degraded_from`，不得静默截断。
6. **工具面不变。**
   不新增第 19 个工具；若实现，作为 `extract` 的可选输入与输出扩展。工具清单仍为 18 个。

## 备选方案

| 方案 | 结论 | 原因 |
|---|---|---|
| 默认开启自动重定位 | 拒绝 | 误报会污染所有调用方；与"零命中是合法空结果"的既有语义冲突 |
| 新增第 19 个工具 `track` | 暂拒绝 | 工具面冻结；一个可选输入即可表达，不需要新工具 |
| 持久化指纹到 state dir | 暂拒绝 | 指纹含页面文本片段，需要过期、脱敏、按域隔离与容量策略；本 spike 未验证 |
| 用嵌入模型做语义相似度 | 拒绝 | 引入模型/依赖/体积与不可解释性；当前文本 Dice + 路径已足够做可行性验证 |
| 基于浏览器渲染后再匹配 | 拒绝 | 违反 ADR-0001；且改版跟踪解决不了 JS 首屏缺失 |

## 后果

- 调用方必须多保存一份指纹（或由上层在两次抽取间传递），并自行确认相似度结果；
- 误报不会进入默认结果，但调用方若忽略 `matched_by` 仍可能误用——文档与输出字段必须显眼；
- 阈值、证据格式与候选结构一旦发布就成为协议面，需要版本化；
- 若未来做持久化，必须另开 ADR 处理隐私与过期，不能在本 ADR 里顺带放行。

## 证据

- 原型：[crates/webctl-web/src/extract/adaptive.rs](../../crates/webctl-web/src/extract/adaptive.rs)（未接线）
- 离线测试：[crates/webctl-web/tests/adaptive.rs](../../crates/webctl-web/tests/adaptive.rs)
- 样本：[crates/webctl-web/tests/fixtures/adaptive/](../../crates/webctl-web/tests/fixtures/adaptive/)
- 结论：[docs/spikes/adaptive-selectors.md](../spikes/adaptive-selectors.md)
