# 从 playwright-crawler-helper 迁移到 webctl

| 项 | 值 |
|---|---|
| 旧项目 | [Thatgfsj-easyRep/playwright-crawler-helper](https://github.com/Thatgfsj-easyRep/playwright-crawler-helper)（Python + Playwright，v3.0.0） |
| 新项目 | webctl（纯 Rust 单二进制，零外部进程、零浏览器） |
| 兼容层 | **不提供**（[PLAN §1 非目标](https://github.com/Thatgfsj/webctl/blob/main/docs/PLAN.md)）。本文是唯一的迁移指引 |
| 依据 | 旧仓源码逐条读取：`ai_crawler.py`（593 行）的 8 个公开函数与其错误码表；webctl v0.4 的 15 个工具与 11 个错误码 |

## 一句话结论

**取回、解析、链接、JSON 这几类能力有对应，但语义被拆开了**（取回与提取分家）；**依赖真实渲染的能力（截图、网络抓包、JS 渲染、滚动、无头/有头切换）明确没有** —— 这是 v0.4 取消浏览器的既定代价（[ADR-0001](adr/0001-single-layer-builtin.md)），不是"还没做"。

## 函数级映射

| 旧 API | 旧行为 | webctl 对应 | 迁移说明 |
|---|---|---|---|
| `fetch(url, sel, headless, wait_until, timeout, screenshot, stealth, capture_network, scroll)` | 真浏览器渲染后取页面，可附带截图/网络抓包/滚动 | `fetch` + `extract` | **拆成两步**：`fetch` 只负责取回（重定向链、cookie、编码、内容类型路由）；`extract` 用 CSS 选择器提取。`headless`/`wait_until`/`stealth`/`scroll` **无对应**（无浏览器）；`screenshot=True` 见下 |
| `crawl(urls, sel, limit, …)` | 批量爬取 + 去重 + 复用浏览器进程 | `fetch` 批量调用 + `paginate` | webctl 不做爬虫调度（PLAN 非目标）：批量循环与去重由调用方维护；`paginate` 只处理单站分页且有页数上限 |
| `links(url, sel, headless, timeout, stealth, same_domain)` | 提取渲染后 DOM 的链接 | `links` | 直接对应；绝对化与 `rel` 由 `links` 处理，`same_domain` 用域名过滤替代。注意：**没有 JS 渲染**，链接来自静态 HTML |
| `json_api(url, timeout)` | 轻量调用 JSON API（不用浏览器） | `fetch` + `json_query` | 两步：`fetch` 按 `application/json` 路由取回，`json_query` 取值/过滤/投影 |
| `stream(urls, sel, …)` | 流式爬取避免大数据集 OOM | `paginate` + 结果上限 | webctl 用"条目/字节上限 + 截断标记 + `LIMIT` 码"替代流式；单次工具调用预算 60s（与 DSH MCP 默认工具超时相等） |
| `close_browser()` | 手动释放浏览器进程 | **不需要** | 没有外部进程与浏览器；会话状态由 `session` 工具管理（PR 05） |
| `screenshot(url, full_page, output_path)` | 页面截图（Base64 或落盘） | **不支持** | 无渲染器 ⇒ 无像素（ADR-0001 代价 2）。没有替代方案，需要截图的流程请留在旧版或改用其它工具 |
| `intercept(url, patterns, wait_actions)` | 拦截并分析页面网络请求 | **不支持** | 需要浏览器网络域。若目标是拿接口数据，改用 `fetch` + `json_query` 直取 API；**SPA 内的接口发现不再支持** |
| `ok(data, meta)` / `err(msg, code, meta)` | 统一信封 | 同形保留 | webctl 沿用 `{ok, data, error, code, meta}`，但收紧三处：①`code` 是**封闭集**（见下表）②`meta` 增加 `engine` / `trust` / `degraded_from` / `stale` / `elapsed_ms` ③**禁止静默降级**（降级必须同时写 `code` 与 `meta.degraded_from`） |

## 错误码迁移

| 旧码 | webctl | 说明 |
|---|---|---|
| `OK` | `OK` | 不变 |
| `TIMEOUT` | `TIMEOUT` | 不变（默认超时值不同，见 PLAN §3.8） |
| `NETWORK` | `NETWORK` | 不变 |
| `BLOCKED` | `BLOCKED` | 不变，且新增**拦截页识别**（Cloudflare/验证码特征） |
| `BAD_URL` | `BAD_URL` | 不变 |
| `TOO_BIG` | **`LIMIT`** | 更名：命中体积或条目上限；结果带截断标记，不再是"整包失败" |
| `PARSE` | `PARSE` | 不变（与"零命中"严格区分：零命中是 `ok=true` 空结果） |
| `HEADLESS` | **无** | "无头模式限制"在取消浏览器后不存在；相关场景归 `BLOCKED`（被拦截）或 `UNSUPPORTED`（需 JS 渲染） |
| `BROWSER_CRASH` | **无** | 没有浏览器进程可崩 |
| （`err` 默认的 `UNKNOWN`） | **`INTERNAL`** | 未知错误不再对外暴露；`INTERNAL` 出现即为缺陷 |
| — | `DEGRADED` | 新增：能力降级但结果仍可用（例如 JS 页面返回原始 HTML） |
| — | `UNSUPPORTED` | 新增：能力不存在（JS 渲染、OCR、扫描件 PDF），**不是失败** |
| — | `SESSION_LOST` | 新增：会话状态丢失或过期 |

## 三类需要另想办法的场景

1. **页面截图** —— 无替代。webctl 不做渲染，也不打算做（ADR-0001/0002）。
2. **接口发现（`intercept` 的核心用法）** —— 直接对目标 API 调 `fetch` + `json_query`；如果只有浏览器里才看得到请求，则 webctl 帮不上。
3. **JS 渲染页面**（旧版最大的差异化能力） —— webctl 会**显式返回 `DEGRADED`/`UNSUPPORTED`**，绝不返回"成功但 0 条"（ADR-0002）。

## 旧仓库处置

- PLAN §10 默认值：归档（archive）旧仓 + 顶部加迁移说明。
- **已执行（2026-10-17）**：
  1. 旧仓 README 顶部加入迁移提示（提交 `898604b`）——指向本仓库、说明**不提供 API 兼容层**，并显式列出"依赖真实渲染的能力不支持"；
  2. 旧仓已**归档**（`archived=true`，只读）：<https://github.com/Thatgfsj-easyRep/playwright-crawler-helper>（归档前该仓 0 个开放 issue / 0 个开放 PR）。
- **撤销方式**（归档可逆）：
  ```bash
  gh api -X PATCH repos/Thatgfsj-easyRep/playwright-crawler-helper -F archived=false
  ```
- 结论与 [PLAN §10](PLAN.md) 的默认值一致：**归档旧版 + 只出迁移映射表**，不提供兼容层。
