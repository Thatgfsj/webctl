# webctl 0.2 路线图（草案，待 0.1.2 收尾后开工）

> 决策依据：[ADR-0006](adr/0006-resolve-site-and-browser-context.md)。本文只写**已获批准**的范围（用户四个决定 + 浏览器上下文注入的原话指示）；未批准的一律不写进这里。

## 1. 目标

用户说得出"网站名"却给不出 URL 时，`webctl` 能给出**带证据与置信度的候选 URL**，并可立刻接现有工具继续取回。

**非目标**：通用网页搜索 / 研究；抓 SERP 作为默认路径；内嵌或驱动浏览器；保证绕过反爬。

## 2. 工具形态（已定：新工具）

第 **17** 个工具 `resolve_site`：

**M1 实际落地的参数**（声明必须等于实现，未接线的参数不进 schema）：

| 参数 | 说明 |
|---|---|
| `site` | 站点名/品牌名（必填）；**不接受 URL**（有 URL 直接用 `fetch`） |
| `max_candidates` | 可选，默认 5，上限 10 |

目标形态里还需要（**M2–M5 落地后**才允许进 schema）：`hint_domain`（已知域名时缩小范围）、`channels`（限制启用哪些通道 `direct`/`site-search`/`public-api`/`sitemap`/`serp-fallback`）、`allow_serp_fallback`（**默认 false**，显式开启 L5）。

输出：`{query, candidates:[{url, score, evidence, channel}], confidence, channel_note}` + 降级时的 `meta.actionable`。

## 3. 浏览器上下文注入（前置能力，独立可用）

- 输入：`user_agent`（必需才生效）/ `accept` / `accept_language` / `sec_ch_ua` / `referer` / `cookies[]`；
- 生效范围：**单次请求**；默认不缓存；`--no-browser-context` 可关；
- 安全：**不落盘、不落日志、不进 envelope**（测试断言）；错误里只出现"已注入 N 条 cookie（已脱敏）"；
- 诚实：**TLS/JS 指纹不可借用** ✗（写进 `PLAN` 与工具描述）。

## 4. 里程碑（每步独立复审 + 门禁 + 文档同步）

| # | 内容 | 依赖 | 验收 |
|---|---|---|---|
| M0 | 浏览器上下文注入 + 脱敏测试 + `--no-browser-context` | 无 | 注入生效；cookie 不出现在任何输出；关掉后行为不变 |
| M1 | `resolve_site` 骨架 + L1 直推 + 内置映射表 | M0 无强依赖 | 常见中英站点名的直推与映射命中率（自测集 ≥20 例）；多候选 + 证据 |
| M2 | L2 站点内建搜索（20–50 条模板） | politeness | robots 拦截时**不请求**并写"是 robots 拦的" |
| M3 | L3 免 key 公共只读 API（Wikidata/Wikipedia） | 无 | 名称→官方站点（P856）；网络不可用时如实降级 |
| M4 | L4 sitemap 声明候选（**需先在 `politeness/robots.rs` 补 `Sitemap:` 解析**——当前只处理 `User-agent`/`Allow`/`Disallow`；**不含** #147/#153 的通道抽取） | #147（离线抽取）与 #153（接进输出）**已合并**；**#117 仍未完成**（拉取与真站 L1 未做） | 只报告声明；不与 L2 混淆 |
| M5 | L5 兜底（默认关，显式 opt-in） | M0 | 未开启时**代码路径不触发**；开启时记日志；拦截即 `BLOCKED` |

## 5. 验收与登记

- 每个里程碑都必须：门禁全绿、独立复审、同步 `PLAN §3.7`/`SKILL.md`/`TESTING.md`；
- **未做到的**（如某站 L1 直推失败）必须写进 `PLAN §9 待验证清单`，不得写成"已支持"；
- 工具面变更（16→17）在 M1 一次性完成，并跑三个守卫（已完成 ✓）。

## 4.1 进度（截至 2026-10-25）

| # | 状态 | 实际做到 / 未做到 |
|---|---|---|
| M0 | **部分** | ✓ 环境变量 `WEBCTL_BROWSER_CONTEXT` 入口 + 脱敏（无 `Serialize`）+ 同源/跨源策略 + e2e 8 条；✗ `--browser-context` / `--no-browser-context` CLI 开关未落地；✗ `fill_form` 的**表单提交**与 CLI `fetch` 不注入（见 `docs/CONFIGURATION.md` 适用范围表） |
| M1 | **完成（仅 L1）** | ✓ `resolve_site`（第 17 个工具）+ 内置表 36 条 + 域名直推 + 全局去重 + 多候选/证据/置信度 + 低置信 `DEGRADED` + `actionable`；✗ 真实站点命中率未验（`PLAN §9` 第 6 条）；✗ L2–L5 未开始 |
| M3 | **完成（L3 通道）** | ✓ 免 key 公共只读 API：Wikidata P856（官方网站），两步请求都过 politeness（robots/限速/429 退避），被 robots 拦下**不请求**且如实回报；endpoint 可注入，6 条离线 fixture 用例钉住请求形状/解析/阻断/显式失败；offline 模式不尝试 L3 |
| M2 / M4 / M5 | **未开始** | L2 站点内建搜索、L4 sitemap（需先补 robots 的 Sitemap 解析）、L5 SERP 兜底（默认关 + 显式 opt-in） |

## 6. 风险

| 风险 | 处置 |
|---|---|
| 兜底抓 SERP 被拦 / 触发风控 | 默认关；显式 opt-in；拦截即 `BLOCKED`；用户已接受 IP 风险（记入 ADR-0006） |
| 内置映射表过时 | 随版本更新；表项带来源与日期；找不到就走后续通道 |
| 公共 API 限流 | 全局限速 + 单次上限；失败如实降级 |
| 名称歧义（同名站点多） | 多候选 + 证据 + 置信度，绝不单选 |