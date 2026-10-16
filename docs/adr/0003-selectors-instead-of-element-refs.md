# ADR-0003：用选择器与结构化路径取代元素引用（ref）

- 状态：已采纳（2026-10-16）
- 关联：[ADR-0001](0001-single-layer-builtin.md)、[PLAN §3.5](https://github.com/Thatgfsj/webctl/blob/main/docs/PLAN.md)

## 背景

v0.3 采纳了 Playwright 式的**元素引用（ref）**：a11y 快照给可交互元素发 `e12` 形式的引用，`role + accessible name` 不变则跨快照复用，失效返回 `STALE_REF`。这套方案的前提是**存在一个持续的 live 页面**——引用只在"最近一次快照"的存活周期内有效。

[ADR-0001](0001-single-layer-builtin.md) 取消浏览器后，这个前提消失了：每次请求都是一次新的取回 + 一次新的解析，不存在"同一个活页面的两次快照"。继续保留 ref 只会制造一个无法兑现的稳定承诺。

## 决策

**用「CSS 选择器 + 结构化路径」取代元素引用。**

| 场景 | 方案 |
|---|---|
| 抽取 | CSS 选择器 + 文本/正则（`extract` / `search`） |
| 表单交互 | `(form_index, field_name)` 稳定寻址；`fill_form` 一次填多字段 |
| 结果可追溯 | 每个命中带结构化路径（tag / 同级索引 / 祖先链）+ 文本上下文 |
| 失效 | **不存在跨请求失效**：每次解析都是新的；`meta.stale` 恒为 false |

并明确写下一条固有语义：**选择器不保证在不同时刻命中同一个元素。**

## 备选方案

| 备选 | 为什么不选 |
|---|---|
| 保留 ref 语义（解析期生成临时 id） | 只能在同一份解析结果内寻址，跨请求无意义；会诱导调用方写出错误的跨调用引用 |
| 位置型 XPath（绝对路径） | SURVEY §8.2 的唯一负面样本：页面轻微变化即失效，且无可解释的失效信号 |
| 裸 `backend_node_id` | 那是 CDP 的浏览器内部标识，v0.4 没有 CDP；SURVEY §8.2 亦记录其复用风险 |

## 后果

- 工具面去掉 `snapshot` / 引用相关的工具，改为 `extract` / `search` / `links` / `tables` / `forms`（PLAN §3.7 的 15 个工具）。
- 抽取结果必须自带"我在页面里的位置"信息，否则调用方无法复核（PR 06 的验收项）。
- 错误码去掉 `STALE_REF`；"零命中"与"解析失败"必须是两个不同的结果（`ok=true` 空结果 vs `code=PARSE`）。

## 证据

- [SURVEY §8.2 元素引用方案对比（PW / CDM / BU / SH 四家）](https://github.com/Thatgfsj/webctl/blob/main/docs/SURVEY.md)。
- [PLAN §3.5](https://github.com/Thatgfsj/webctl/blob/main/docs/PLAN.md)。
