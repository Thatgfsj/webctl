# ADR-0002：本期不做 JS 引擎，JS 页面显式降级

- 状态：已采纳（2026-10-16）
- 关联：[ADR-0001](0001-single-layer-builtin.md)、[PLAN §10 开放决策 #2](https://github.com/Thatgfsj/webctl/blob/main/docs/PLAN.md)

## 背景

[ADR-0001](0001-single-layer-builtin.md) 取消了外部浏览器，于是没有任何 JS 引擎。但"没有引擎"不等于"可以不表态"：如果对 JS 页面返回 `ok=true` 且 0 条结果，调用方（AI agent）会把"抓不到"误当成"页面确实没有内容"——这是最危险的失败模式。

## 决策

**v0.4 不内置任何 JS 引擎。** 对依赖 JS 渲染的页面，必须返回 `DEGRADED`（能给出部分内容时）或 `UNSUPPORTED`（完全无法处理时），并在 `error` 里说明判定依据（脚本密度 / 空容器 / `<noscript>` / 框架指纹）。

保留 `meta.engine` 字段（当前恒为 `"builtin"`）作为将来的缝隙。

## 备选方案

| 备选 | 取舍 |
|---|---|
| Boa（纯 Rust JS 引擎）+ 自建最小 DOM/BOM | 保持"纯 Rust、无 C 依赖"，但 DOM/BOM/fetch 全要自己实现，等于自研 mini-browser；兼容性与性能远不及 V8，复杂框架（React/Vue 水合）大概率跑不动 |
| rquickjs（QuickJS + C 依赖）+ 自建 DOM | 兼容性与速度优于 Boa，仍是单文件静态二进制；但引入 C 工具链，且 musl 静态交叉编译更麻烦（BoringSSL 已有同类问题，见 `docs/spikes/wreq.md` §7） |
| 调外部 headless 浏览器 | 与 ADR-0001 直接冲突 |
| 本期不做 | ✅ 采用 |

## 后果

- 验收锚点从"JS 页必须拿到 10 条"改为**诚实降级锚点**：JS 页必须返回 `DEGRADED`/`UNSUPPORTED`，**不得**返回"成功但 0 条"；静态等价锚点仍是 10 条。
- `SPA/JS 降级语义`（PR 09）成为一个独立 PR，而不是某个功能的分支逻辑。
- 将来引入引擎时，本 ADR 被新 ADR 取代，且必须先给出"内置引擎 vs 继续降级"的成本对比。

## 证据

- [SURVEY §2.4](https://github.com/Thatgfsj/webctl/blob/main/docs/SURVEY.md)：`quotes.toscrape.com/js/` 静态抓取 0 条，静态版 10 条。
- [PLAN §5.3 验收锚点](https://github.com/Thatgfsj/webctl/blob/main/docs/PLAN.md)（v0.4 重定）。
- 本决策是"未回复则按默认值执行"下的**默认值**（维护者在被问及 JS 引擎选型时未作答，取最保守且可逆的一项）。
