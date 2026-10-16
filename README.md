<a name="readme-top"></a>

# webctl

**给 AI agent 用的网页控制层：纯 Rust 单静态二进制，零外部进程、零浏览器，内置取回/解析/会话/文档能力，提供 CLI 与 MCP server。**

[![CI](https://img.shields.io/github/actions/workflow/status/Thatgfsj/webctl/ci.yml?label=CI&style=flat-square)](https://github.com/Thatgfsj/webctl/actions/workflows/ci.yml)
[![状态](https://img.shields.io/badge/status-pre--release-orange?style=flat-square)](#项目状态)
[![许可证](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.98.1-dea584?style=flat-square)](rust-toolchain.toml)

[项目状态](#项目状态) · [安装](#安装) · [快速开始](#快速开始) · [能力与现状](#能力与现状) · [English](#english) · [计划](docs/PLAN.md) · [发布流程](docs/RELEASE.md) · [贡献指南](CONTRIBUTING.md) · [议题](https://github.com/Thatgfsj/webctl/issues)

> **尚无可用发布版。** 本仓库还没有任何 tag / release，因此没有可用的下载或安装命令（[诚实安装说明](#安装)）。
> 首个版本将由 [cargo-dist 发布流水线](docs/RELEASE.md)产出；当前请从源码构建（[快速开始](#快速开始)）。进度见 [PLAN §5.2.1](docs/PLAN.md)。

---

## 项目状态

| 项 | 值 |
|---|---|
| 阶段 | P0 进行中：骨架、取回、解析、会话、状态、文档/数据、礼貌限速、CLI 已合并；抽取（PR 06）、降级（PR 09）、MCP（PR 11）、安全（PR 12）与发布（PR 14）推进中 |
| 发布 | **无**：尚无 tag / release；发布流水线已就位（[docs/RELEASE.md](docs/RELEASE.md)），资产一致性待首个 release 核验（PKG-06/07） |
| 计划文档 | [docs/PLAN.md](docs/PLAN.md)（**v0.4**，含 2026-10-16 的方向变更） |
| 事实依据 | [docs/SURVEY.md](docs/SURVEY.md)（本机实测 + 公开仓库核对；其中 CDP 部分已成历史记录） |
| 形态 | Rust workspace，4 个 crate，单个静态二进制 |
| 接口 | CLI（当前 `caps` / `fetch` / `config` 三个子命令）+ MCP server（`mcp__webctl__*`，由 PR 11 落地） |
| 外部依赖 | **运行时零外部进程、零浏览器**；不调用 `curl`/`jq`/`pdftotext` 等系统命令 |
| 发布物 | cargo-dist 生成的三平台压缩包 + shell/PowerShell 安装器（[清单](docs/RELEASE.md)）；**当前尚未发布** |
| 前身 | [playwright-crawler-helper](https://github.com/Thatgfsj-easyRep/playwright-crawler-helper)（Python + Playwright，不再兼容维护） |
| 许可证 | [MIT](LICENSE) |

## 能力与现状

一套能力，全部**在进程内**实现（[计划 §2](docs/PLAN.md)）。下表如实标注合并进度——未合并的能力不算已支持：

| 能力域 | 内容 | 现状 |
|---|---|---|
| **取回** | HTTP(S)、重定向链、cookie jar、编码/压缩、内容类型路由、并发与限速、robots | ✅ 已合并（PR 02 / 08） |
| **解析** | HTML→DOM、正文提取、链接、表格、表单、JSON/XML、markdown | ✅ 已合并（PR 03） |
| **会话** | cookie 持久化、自动填表提交、CSRF、multipart、分页、登录态导入导出 | ✅ 已合并（PR 04 / 05） |
| **文档/数据** | PDF 文本层、JSON 查询子集、HTML→文本（`pdftotext`/`pandoc`/`jq` 的内置替代） | ✅ 已合并（PR 07） |
| **CLI** | 配置优先级、代理、输出格式、退出码、`caps`/`fetch`/`config` | ✅ 已合并（PR 10） |
| **抽取** | 选择器 / `search` / `links` / `tables` / `forms` / `paginate` 的结构化输出 | ⏳ PR 06 |
| **降级语义** | SPA/JS 页显式 `DEGRADED`/`UNSUPPORTED`，禁止"成功但 0 条" | ⏳ PR 09 |
| **MCP** | rmcp server + 15 工具 schema + `SKILL.md` | ⏳ PR 11 |
| **安全** | cookie 导出确认、信封/日志脱敏、路径与协议校验 | ⏳ PR 12 |

工具面定稿为 **15 个内置工具**（[PLAN §3.7](docs/PLAN.md)）：
`fetch` / `extract` / `search` / `links` / `tables` / `forms` / `fill_form` / `paginate` /
`session` / `cookies` / `json_query` / `doc_text` / `download` / `robots` / `caps`。
**当前 CLI 只暴露 `caps` / `fetch` / `config`**；15 个工具由 PR 11 以 MCP 工具面落地。

<!-- 待 #68 / PR 11 合并后由 caps --json 回填：以 webctl caps --json 的真实输出（枚举 15 个工具 + 诚实可用性）替换本节的"结构 + 已实现部分"，并与 PLAN §3.7 逐项对照。 -->

## 它不做什么（诚实清单）

v0.4 **取消了外部浏览器与外部 CLI**（[计划 §0](docs/PLAN.md)）。下面这些是**明确不支持**，不是"以后再说"：

- **不执行 JS、不渲染 SPA**：JS 注入内容与首屏水合拿不到 → 显式返回 `DEGRADED`/`UNSUPPORTED`，**绝不伪装成"成功但 0 条"**。
- **没有截图、没有真实布局几何**：没有渲染器就没有像素，`screenshot`、页面 PDF 打印、按坐标点击都不存在。
- **没有 OCR**：纯 Rust 无成熟 OCR；扫描件 PDF 只给 `UNSUPPORTED`。
- **不承诺绕过 Cloudflare/验证码**：识别到拦截页即 `BLOCKED` 并给出诊断，只提供 TLS 指纹、合规请求头、限速与退避。

## 安装

> ⚠️ **尚无发布版。** 本仓库没有任何 tag / release，所以**现在没有**可用的下载 / 安装命令，README 也不编造这些命令。
> 首个 release 之后，安装方式将是 cargo-dist 生成的 `webctl-installer.sh`（Linux/macOS）、`webctl-installer.ps1`（Windows）
> 与三平台压缩包；精确资产名与安装命令见 [docs/RELEASE.md §2](docs/RELEASE.md)。**当前唯一方式是[从源码构建](#快速开始)。**

## 快速开始

~~~bash
git clone https://github.com/Thatgfsj/webctl.git
cd webctl

cargo build --release        # 需要 cmake + C/C++ + libclang，见「构建」
./target/release/webctl --version      # webctl 0.1.0
./target/release/webctl caps --json    # 能力自检：不联网、不探测外部程序
./target/release/webctl fetch https://example.com/
~~~

## 构建

~~~bash
cargo build --release   # Rust 1.98.1（固定于 rust-toolchain.toml）
cargo test              # 全部离线：只连 127.0.0.1，不访问外网
cargo run -- caps --json
~~~

**运行时零依赖**（单二进制、零外部进程、零浏览器）；下面这些**只在编译期**需要：

| 构建期需求 | 为什么 | 怎么办 |
|---|---|---|
| `cmake` + C/C++ 编译器 | 取回层用 `wreq`，默认 TLS 后端是 BoringSSL（`btls-sys`） | 发行版装 `cmake` + `build-essential` |
| **libclang** + clang 内建头 | BoringSSL 绑定由 bindgen 生成 | 装 `libclang-dev` / `clang`；最小环境可设 `LIBCLANG_PATH` 与 `BINDGEN_EXTRA_CLANG_ARGS` |

缺 libclang 的典型报错是 `Unable to find libclang`；只给到 `.so` 而缺头文件时报 `'stddef.h' file not found`。完整报错、修复命令与本机实测（首次全量构建 76s，2026-10-17 release 全量 1m52s）见 [docs/spikes/wreq.md](docs/spikes/wreq.md) §4。

GitHub Actions 的 ubuntu / macOS / Windows 镜像**自带**上述依赖，无需额外步骤（main 上三平台 CI 全绿）。

## 配置与 CLI

CLI 配置按 **CLI 参数 > 环境变量 > 配置文件 > 内置默认** 合并。代理只认 `WEBCTL_PROXY` / 配置文件 `proxy`；宿主 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` 一律忽略，未配置即直连。JSON 输出恒为单行，错误码 → 退出码为稳定映射。

~~~bash
webctl caps --json
webctl --config ./webctl.toml fetch https://example.com/
webctl --offline caps
~~~

当前子命令只有 `caps` / `fetch` / `config`；环境变量清单、配置文件格式与完整退出码表见 [docs/CONFIGURATION.md](docs/CONFIGURATION.md)。

## 发布与体积基线

- 发布步骤、release note 模板、变更日志约定与发布 E2E 清单：[docs/RELEASE.md](docs/RELEASE.md)。
- 体积 / 冷启动基线工具：`scripts/size-baseline.sh`；记录：[docs/size-baseline.json](docs/size-baseline.json)。
  - glibc release **本机实测** 7,978,256 字节（约 7.61 MiB），`--version` 冷启动 p50 约 10–24 ms（全部 < 50 ms，CLI-12 本地通过）。
  - **musl 数字待 CI 回填**（本机未安装 musl target）。
- 发布资产 E2E 冒烟：`scripts/release-e2e.sh`（PKG-04 / PKG-06 / PKG-08）。

## 架构

~~~
crates/
├─ webctl/        # bin：CLI 入口 + MCP 入口（feature 切换）
├─ webctl-core/   # envelope / 错误码 / 配置 / 能力自检 / 会话状态 / 安全脱敏
├─ webctl-web/    # 取回 + 解析 + 会话 + 文档与数据（全部内置）
└─ webctl-mcp/    # MCP server + 工具 schema + SKILL.md（PR 11；当前为骨架）
~~~

拆分纪律：**只有变更来源/节奏不同、且依赖方向单向无环时才拆 crate**；其余同一 crate 内用目录分隔。

## 建设路线

路线图按 PR 切片，每个 PR 一个 issue（先领 issue 再动手）：

- 全部议题：[Thatgfsj/webctl issues](https://github.com/Thatgfsj/webctl/issues)
- 里程碑：P0 骨架与基线 → 内置取回与解析 → 会话与状态 → 抽取与文档 → MCP 工具面 → 发布
- 验收锚点（v0.4 重定）：静态版 `quotes.toscrape.com/` 必须稳定取到 **10 条**；JS 版必须返回 `DEGRADED` 而非"成功但 0 条"。

## 参与贡献

先读 [CONTRIBUTING.md](CONTRIBUTING.md)：**feat 前先写 issue 并领取批准**、**一个 PR 只做一类变更**、**单个 PR ≤1000 行**（口径与排除项见该文件）。

历史探针脚本在 [probes/cdp/](probes/cdp/)：它们记录的是 v0.3 时代"手写 CDP 驱动 Edge"的实测结果，**v0.4 已不再走这条路线**，保留仅为可复查的历史证据。

## English

**webctl is a web-control layer for AI agents** — a single pure-Rust static binary with everything built in: no external processes, no browser, no JS engine. It provides an HTTP fetch engine, HTML/JSON/XML/PDF extraction, a protocol-level session engine, and a CLI plus an MCP server.

- **Status:** pre-release. There are no tags or releases yet, so no install command is advertised here. The release pipeline (cargo-dist, Linux/macOS/Windows + static musl) is in place — see [docs/RELEASE.md](docs/RELEASE.md); build from source in the meantime.
- **Runtime:** zero subprocesses, zero browser, zero JS execution. JS-heavy pages degrade explicitly to `DEGRADED`/`UNSUPPORTED` instead of silently returning "success with 0 results".
- **Capabilities shipped so far:** fetch (PR 02/08), HTML parsing (PR 03), session/form engine (PR 04/05), PDF & JSON data (PR 07), CLI and config (PR 10). Extraction, MCP, and security hardening are still in progress; the README table above is the source of truth for status.
- **Commands available today:** `webctl caps`, `webctl fetch <url>`, `webctl config`. The 15-tool surface (MCP) lands with PR 11.
- **Build:** `cargo build --release` (requires cmake, a C/C++ toolchain, and libclang at build time only). Rust 1.98.1, pinned in `rust-toolchain.toml`.
- **Docs:** [PLAN](docs/PLAN.md) · [RELEASE](docs/RELEASE.md) · [TESTING](docs/TESTING.md) · [CONTRIBUTING](CONTRIBUTING.md).
- **License:** MIT.

## 许可证

[MIT](LICENSE) © 2026 Thatgfsj

[↑ 回到顶部](#readme-top)
