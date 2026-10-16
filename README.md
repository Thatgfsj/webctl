<a name="readme-top"></a>

# webctl

**给 AI agent 用的网页与浏览器控制层 —— 纯 Rust 单静态二进制，同时是 CLI 与 MCP server：有浏览器时用自研 CDP 直接驱动 Edge/Chrome，没有浏览器时用内置 HTTP/解析 + 协议级会话引擎顶上。**

[![状态](https://img.shields.io/badge/status-design%20phase-orange?style=flat-square)](#项目状态)
[![许可证](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
[![计划](https://img.shields.io/badge/docs-PLAN.md-informational?style=flat-square)](docs/PLAN.md)

[项目状态](#项目状态) · [计划](docs/PLAN.md) · [调查证据](docs/SURVEY.md) · [贡献指南](CONTRIBUTING.md) · [议题](https://github.com/Thatgfsj/webctl/issues) · [探针](probes/README.md)

> **最新版：尚无可用构建。** 本仓库处于 P0 之前的**设计阶段**：计划与调查证据已就绪，代码骨架待 PR 01 落地。

---

## 项目状态

| 项 | 值 |
|---|---|
| 阶段 | 设计阶段（P0 未开工） |
| 计划文档 | [docs/PLAN.md](docs/PLAN.md)（草案 v0.3） |
| 事实依据 | [docs/SURVEY.md](docs/SURVEY.md)（本机实测 + 公开仓库核对，2026-10-16） |
| 形态 | Rust workspace，7 个 crate，单个静态二进制 |
| 接口 | CLI + MCP server（`mcp__webctl__*`） |
| 前身 | [playwright-crawler-helper](https://github.com/Thatgfsj-easyRep/playwright-crawler-helper)（Python + Playwright，不再兼容维护） |
| 许可证 | [MIT](LICENSE) |

## 它解决什么

现有浏览器自动化要么把 Playwright/WebDriver 这类重运行时塞进 AI 服务，要么只能抓静态 HTML。webctl 的支点是**一套命令面、四种后端**：同一命令在不同环境下语义一致，只变 `meta.tier`，agent 工作流不用改写。

| 层 | 名称 | 依赖 | 能力 |
|---|---|---|---|
| **T0** | 内置解析 | 纯 Rust（wreq + scraper + dom_smoothie） | 拉取、正文、链接、表格、表单、JSON/XML/PDF 文本层 |
| **T1** | 协议级会话引擎 | 纯 Rust | cookie jar、重定向链、自动填表、CSRF、multipart、分页、登录态导入导出 |
| **T2** | 文档桥 | 系统 CLI 白名单 | `pdftotext` / `tesseract` / `pandoc` / `jq`（有则启用） |
| **T3** | 真浏览器 | 自研 CDP 客户端 | 导航、a11y 快照、点击/输入/等待、JS 求值、截图/PDF、网络抓包、多标签 |

设计约束（详见 [PLAN §3](docs/PLAN.md)）：**禁止静默降级**（降级必须写进 `meta.degraded_from`）、页面内容一律标 `trust: untrusted_page_content`、`eval` 默认关闭、不执行任意 shell 字符串、不承诺绕过 Cloudflare/验证码。

## 架构

```
crates/
├─ webctl/            # bin：CLI 入口 + MCP 入口（feature 切换）
├─ webctl-core/       # envelope / 错误码 / 配置 / 能力探测 / tier 路由 / 会话与状态
├─ webctl-web/        # T0 + T1：HTTP、编码、cookie、解析、正文、表单、协议会话引擎、T2 白名单桥
├─ webctl-cdp/        # T3 底座：线协议信封、传输、分发、事件、target 会话、浏览器发现与启动
├─ webctl-cdp-types/  # T3：手写最小协议类型（唯一因"来源不同 + 体量"而拆的 crate）
├─ webctl-browser/    # T3 语义层：快照、交互原语、截图/PDF、网络捕获
└─ webctl-mcp/        # rmcp server + 工具 schema + SKILL.md 生成
```

拆分纪律：**只有变更来源/节奏不同、且依赖单向无环时才拆 crate**；其余同 crate 内用目录分隔。工具面定稿为 [核心 20 个](docs/PLAN.md)（navigate / snapshot / find / click / fill_form / extract …）。

## 建设路线

路线图按 PR 切片，每个 PR 一个 issue（先领 issue 再动手）：

- 全部议题：[Thatgfsj/webctl issues](https://github.com/Thatgfsj/webctl/issues)
- 里程碑：P0 骨架 → T0 内置解析 → T1 会话引擎 → T3 CDP 客户端 → MCP 工具面 → 发布
- 验收锚点：`quotes.toscrape.com/js/` 静态抓取 0 条 → webctl 必须拿到 **10 条**（[SURVEY §2.4](docs/SURVEY.md)）

## 参与贡献

先读 [CONTRIBUTING.md](CONTRIBUTING.md)：**feat 前先写 issue 并领取批准**、**一个 PR 只做一类变更**、**单个 PR ≤1000 行**（口径与排除项见该文件）。

历史探针脚本（可原地重跑，复现 SURVEY 的 CDP 实测）在 [probes/cdp/](probes/cdp/)。

## 许可证

[MIT](LICENSE) © 2026 Thatgfsj

[↑ 回到顶部](#readme-top)
