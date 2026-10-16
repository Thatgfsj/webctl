<a name="readme-top"></a>

# webctl

**给 AI agent 用的网页控制层 —— 纯 Rust 单静态二进制，零外部进程、零浏览器依赖：内置 HTTP 取回、解析抽取、协议级会话引擎与文档处理，同时提供 CLI 与 MCP server。**

[![状态](https://img.shields.io/badge/status-design%20phase-orange?style=flat-square)](#项目状态)
[![许可证](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
[![计划](https://img.shields.io/badge/docs-PLAN.md-informational?style=flat-square)](docs/PLAN.md)

[项目状态](#项目状态) · [计划](docs/PLAN.md) · [调查证据](docs/SURVEY.md) · [贡献指南](CONTRIBUTING.md) · [议题](https://github.com/Thatgfsj/webctl/issues)

> **尚无可用发布版。** 计划 v0.4、骨架（PR 01）与取回层（PR 02）已合并；解析 / 会话 / 文档 / MCP 等按 [PLAN §5.2](docs/PLAN.md) 推进。

---

## 项目状态

| 项 | 值 |
|---|---|
| 阶段 | P0 进行中：骨架 + 取回层已合并，其余按 PLAN §5.2 推进 |
| 计划文档 | [docs/PLAN.md](docs/PLAN.md)（**草案 v0.4**，含 2026-10-16 的方向变更） |
| 事实依据 | [docs/SURVEY.md](docs/SURVEY.md)（本机实测 + 公开仓库核对；其中 CDP 部分已成历史记录） |
| 形态 | Rust workspace，4 个 crate，单个静态二进制 |
| 接口 | CLI + MCP server（`mcp__webctl__*`） |
| 外部依赖 | **运行时零外部进程、零浏览器**；不调用 `curl`/`jq`/`pdftotext` 等系统命令 |
| 前身 | [playwright-crawler-helper](https://github.com/Thatgfsj-easyRep/playwright-crawler-helper)（Python + Playwright，不再兼容维护） |
| 许可证 | [MIT](LICENSE) |

## 它做什么

一套能力，全部**在进程内**实现（[计划 §2](docs/PLAN.md)）：

| 能力域 | 内容 |
|---|---|
| **取回** | HTTP(S)、重定向链、cookie jar、编码/压缩、内容类型路由、并发与限速 |
| **解析** | HTML→DOM、正文提取、链接、表格、表单、JSON/XML、markdown |
| **会话** | cookie 持久化、自动填表提交、CSRF、multipart、分页、登录态导入导出 |
| **文档/数据** | PDF 文本层、JSON 查询子集、HTML→文本（`pdftotext`/`pandoc`/`jq` 的内置替代） |
| **接口** | CLI 与 MCP，同一套命令面；统一结果信封与明确降级语义 |

工具面定稿为 **15 个内置工具**（fetch / extract / search / links / tables / forms / fill_form / paginate / session / cookies / json_query / doc_text / download / robots / caps）。

## 它不做什么（诚实清单）

v0.4 **取消了外部浏览器与外部 CLI**（[计划 §0](docs/PLAN.md)）。下面这些是**明确不支持**，不是"以后再说"：

- **不执行 JS、不渲染 SPA**：JS 注入内容与首屏水合拿不到 → 显式返回 `DEGRADED`/`UNSUPPORTED`，**绝不伪装成"成功但 0 条"**。
- **没有截图、没有真实布局几何**：没有渲染器就没有像素，`screenshot`、页面 PDF 打印、按坐标点击都不存在。
- **没有 OCR**：纯 Rust 无成熟 OCR；扫描件 PDF 只给 `UNSUPPORTED`。
- **不承诺绕过 Cloudflare/验证码**：识别到拦截页即 `BLOCKED` 并给出诊断，只提供 TLS 指纹、合规请求头、限速与退避。

## 架构

```
crates/
├─ webctl/        # bin：CLI 入口 + MCP 入口（feature 切换）
├─ webctl-core/   # envelope / 错误码 / 配置 / 能力自检 / 会话状态 / 安全脱敏
├─ webctl-web/    # 取回 + 解析 + 会话 + 文档与数据（全部内置）
└─ webctl-mcp/    # rmcp server + 工具 schema + SKILL.md 生成
```

拆分纪律：**只有变更来源/节奏不同、且依赖方向单向无环时才拆 crate**；其余同一 crate 内用目录分隔。

## 构建

```bash
cargo build --release   # Rust 1.98.1（固定于 rust-toolchain.toml）
cargo test              # 全部离线：只连 127.0.0.1，不访问外网
cargo run -- caps --json
```

**运行时零依赖**（单二进制、零外部进程、零浏览器）；下面这些**只在编译期**需要：

| 构建期需求 | 为什么 | 怎么办 |
|---|---|---|
| `cmake` + C/C++ 编译器 | 取回层用 `wreq`，默认 TLS 后端是 BoringSSL（`btls-sys`） | 发行版装 `cmake` + `build-essential` |
| **libclang** + clang 内建头 | BoringSSL 绑定由 bindgen 生成 | 装 `libclang-dev` / `clang`；最小环境可设 `LIBCLANG_PATH` 与 `BINDGEN_EXTRA_CLANG_ARGS` |

缺 libclang 的典型报错是 `Unable to find libclang`；只给到 `.so` 而缺头文件时报 `'stddef.h' file not found`。完整报错、修复命令与本机实测（首次全量构建 76s）见 [docs/spikes/wreq.md](docs/spikes/wreq.md) §4。

GitHub Actions 的 ubuntu / macOS / Windows 镜像**自带**上述依赖，无需额外步骤（main 上三平台 CI 全绿）。

## 配置与 CLI

CLI 配置按 **CLI 参数 > 环境变量 > 配置文件 > 内置默认** 合并。代理只认 `WEBCTL_PROXY` / 配置文件 `proxy`；宿主 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` 一律忽略，未配置即直连。JSON 输出恒为单行，错误码 → 退出码为稳定映射。

```bash
webctl caps --json
webctl --config ./webctl.toml fetch https://example.com/
webctl --offline caps
```

环境变量清单、配置文件格式与完整退出码表见 [docs/CONFIGURATION.md](docs/CONFIGURATION.md)。

## 建设路线

路线图按 PR 切片，每个 PR 一个 issue（先领 issue 再动手）：

- 全部议题：[Thatgfsj/webctl issues](https://github.com/Thatgfsj/webctl/issues)
- 里程碑：P0 骨架与基线 → 内置取回与解析 → 会话与状态 → 抽取与文档 → MCP 工具面 → 发布
- 验收锚点（v0.4 重定）：静态版 `quotes.toscrape.com/` 必须稳定取到 **10 条**；JS 版必须返回 `DEGRADED` 而非"成功但 0 条"。

## 参与贡献

先读 [CONTRIBUTING.md](CONTRIBUTING.md)：**feat 前先写 issue 并领取批准**、**一个 PR 只做一类变更**、**单个 PR ≤1000 行**（口径与排除项见该文件）。

历史探针脚本在 [probes/cdp/](probes/cdp/)：它们记录的是 v0.3 时代"手写 CDP 驱动 Edge"的实测结果，**v0.4 已不再走这条路线**，保留仅为可复查的历史证据。

## 许可证

[MIT](LICENSE) © 2026 Thatgfsj

[↑ 回到顶部](#readme-top)
