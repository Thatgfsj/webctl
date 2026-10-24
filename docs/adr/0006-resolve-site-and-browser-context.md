# ADR-0006：站点定位兜底与浏览器上下文注入

- 状态：草案（2026-10-20，待 0.2 实现）
- 关联：[ADR-0004](0004-politeness-by-default.md)、[PLAN §1 目标 1「无浏览器」](../PLAN.md)、[ROADMAP-0.2](../ROADMAP-0.2.md)

## 背景

用户（agent）常常知道"要哪个网站"，但拿不到 URL。没有 URL，现有的取回/解析工具都无从下手。

同时，本项目此前把"零外部浏览器"写成了硬约束（[ADR-0001](0001-single-layer-builtin.md)）。用户明确要求两处调整，**原话**：

1. 「零外部浏览器改为尝试驱动 llm 读取设备浏览器信息再注入我们的工具侧」—— 即不追求内嵌/驱动浏览器，但允许 LLM 从设备浏览器读取上下文并注入工具侧；
2. **家庭宽带 IP 的风控风险由用户明示接受**（本 ADR 记入取舍，不再作为阻塞理由）。

另据用户决定：**不引入商业搜索 API 后端**、**不引入自托管外部服务（如 SearXNG）**；但**允许免 key 的公共只读 API**（Wikidata/Wikipedia）作为 L3 通道。

## 决策

1. **新增第 17 个工具 `resolve_site`**：输入站点名（或关键词），输出**带证据与置信度**的候选 URL 列表；不做通用网页搜索。
2. **通道阶梯**（按零外部依赖优先，且每层必须给出"判定依据"）：
   - L1 URL 直推 + **内置常见站点映射表**（用户已批准；纯本地、随版本更新）；
   - L2 **站点内建搜索**（已知域名时用其搜索入口，**先过 politeness**）；
   - L3 **免 key 公共只读 API**（Wikidata/Wikipedia；用户已批准）；
   - L4 **sitemap 声明**候选：来源将是 `politeness/robots.rs` 的 `Sitemap:` 指令——**该解析尚未实现**（当前 `parse()` 只处理 `User-agent`/`Allow`/`Disallow`，`Sitemap:` 落入 `_ => {}` 被忽略），M4 必须先补解析；**与** `#147/#153` 的通道抽取（RSS/Atom/oEmbed 与 `og:*`/`twitter:*` 元数据，`site_channels.rs` **不含 sitemap**）**无关**；
   - L5 **用户出口抓 SERP**（`WEBCTL_PROXY` + 注入的浏览器上下文）：**默认关闭、每次显式 opt-in**（用户已批准该参数形态）。
3. **浏览器上下文注入**：webctl **不读取、不驱动**浏览器；只接受由宿主/LLM 提供的 `user_agent / accept / accept_language / sec_ch_ua / referer / cookies`，**按单次请求生效**、**绝不落盘/落日志/进 envelope**（**实现时必须补测试断言**；当前仓库尚无该代码）。
4. **诚实边界（写进文档与错误信息）**：注入能借用浏览器的 **HTTP 头与 Cookie** ✓，但 **TLS 指纹仍是 wreq 的** ✗、**JS/WebGL/字体指纹仍不存在** ✗ —— 这不是"伪装成浏览器"，只是"携带用户会话" ✗；**不保证**绕过任何反爬 ✓，被拦即 `BLOCKED` ✓。

## 备选方案（以及为什么没选）

| 方案 | 未采纳原因 |
|---|---|
| 商业搜索 API（Brave/CSE/Bing…） | 用户明确否决（不要后端） |
| 自托管 SearXNG | 用户明确否决（不要外部服务） |
| 内嵌/驱动真实浏览器 + 指纹伪装 | 与 ADR-0001 的形态冲突、维护成本高、成功率不可承诺 |
| 只用 L1 直推 | 中文站点名（如"知乎"）几乎必然失败；必须有映射表与 L2/L3 兜底 |

## 后果

- 工具面 **16 → 17**：真源是 `crates/webctl-mcp/src/tools.rs` 的 `TOOL_NAMES`（`[&str; 17]`）与 `TOOL_SPECS`；`scripts/check-tool-face.py` 还会校验 `SKILL.md` 的标题与 `tools:start..end` 清单、`README.md` 的名单与逐名出现，并解析 `PLAN §3.7` 的**表格**（行数/名字/顺序；PLAN.md 不在其 SCAN_FILES 里，标题不单独校验）。**因此同步清单必须包含**：`TOOL_NAMES`/`TOOL_SPECS`、`SKILL.md`、`README.md`、`PLAN §3.7`，否则守卫会红。
- `PLAN §1` 的形态约束在 `docs/PLAN.md:7`「零外部进程、零外部浏览器依赖」与 `docs/PLAN.md:48`（§1 目标 1：纯 Rust、无 Python/Node/浏览器）；本 ADR 提议把这些表述**补充**为"浏览器不作为依赖；可注入宿主提供的浏览器上下文"——**该措辞是新增的，不是 PLAN 现有文字**，不得反向引用为 PLAN 原文。
- `resolve_site` **绝不返回"看起来像"的单一答案**：多候选 + 证据 + 置信度，低置信时给 `actionable.next_steps`；
- L5 兜底必须**默认关**、日志留痕"已启用兜底"、失败如实 `BLOCKED`。

## 证据

- 用户决策来源：本会话用户明确回复（四个决定 + 浏览器上下文注入的原话指示），并见 [ROADMAP-0.2](../ROADMAP-0.2.md)；
- 现状依据：[PLAN §1](../PLAN.md)（无浏览器的形态约束）、[ADR-0004](0004-politeness-by-default.md)、[ROADMAP-0.2 §4](../ROADMAP-0.2.md)；
- **本 ADR 为草案**：实现完成前，文中"实现时必须补测试断言"等条目**不得**被引用为已实现。
- **2026-10-25 补记（M0 部分 + M1 仅 L1 已落地）**：浏览器上下文为环境变量入口（`WEBCTL_BROWSER_CONTEXT`）；`resolve_site` 已作为第 17 个工具上线，但**只有 L1**（内置表 + 直推），L2–L5 未实现；未落地项已逐条登记在 [PLAN §9](../PLAN.md) 与 [ROADMAP-0.2 §4.1](../ROADMAP-0.2.md)。
- **2026-10-25 补记（M3 落地）**：L3 通道（Wikidata P856 官方网站）已实现并通过离线 fixture 用例；**每次请求都过 politeness**（robots/限速/429 退避），被 robots 拦下不请求且如实回报；L2 仍未实现。
- **2026-10-25 补记（M5 落地）**：L5 兜底已实现但**默认关闭**——只有调用方写 `allow_serp_fallback=true` 才抓 SERP；结果固定低分（45 < 可靠阈值 60）、被 robots 拦下不请求、请求留痕到 stderr，不存在『默认就去抓搜索结果』的路径。
- **2026-10-25 补记（M4 落地）**：`politeness/robots.rs` 已解析 `Sitemap:`（此前的 `_ => {}` 分支）；L4 通道对直推候选探测 robots.txt 并**只报告声明**——『站点在运营』不等于『就是用户说的那个站』，因此不据此提升置信度，也不抓 sitemap 内容（那属 #147/#153 的通道抽取）。