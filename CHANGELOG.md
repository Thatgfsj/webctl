# 变更日志

本文件遵循 [Keep a Changelog 1.1.0](https://keepachangelog.com/zh-CN/1.1.0/) 与
[语义化版本](https://semver.org/lang/zh-CN/)。格式约定、发版步骤与 release note 模板见
[docs/RELEASE.md](docs/RELEASE.md)。

## [Unreleased]

### Added

- **第 18 个工具 `web_search`（0.2.2 M1，[#200](https://github.com/Thatgfsj/webctl/issues/200)）**：按查询词全网搜索，默认 **Bing**（HTML SERP）；返回结果卡片（title/url/snippet/rank/engine）与逐引擎状态，**不自动抓正文**（正文走 fetch/extract）；每次请求前 **50–500ms 随机抖动**，带简单反爬头（不做验证码/Cloudflare/代理）；被拦 / 零结果 / 超时显式 `DEGRADED` + `actionable`（`search-blocked` / `search-empty` / `search-timeout`）；结果（以及 M2 起的摘要）按不可信页面内容处理，`notes` 明示 Bing 的 robots 事实，`WEBCTL_SEARCH_STRICT_ROBOTS=1` 可切严格模式。

## [0.2.1] - 2026-10-29

### Added

- **站点级通道取回**（[#117](https://github.com/Thatgfsj/webctl/issues/117)）：`embedded_data` 新增 `fetch_channels`（**默认 false**）——把页面声明的 RSS/Atom 通道过 politeness 取回并解析条目（每次最多 2 个通道、每通道 10 条）；robots 拦下**不请求**并逐通道如实报 `blocked`；全失败显式 `DEGRADED` + `channel-fetch-failed` + `feed-unavailable`；oEmbed 只报声明不拉取；元数据/条目与正文严格分开（`source` 区分）。
- **浏览器上下文 CLI 开关**（[#183](https://github.com/Thatgfsj/webctl/issues/183)）：`--browser-context <PATH>` / `--no-browser-context`（互斥；显式开关优先于 `WEBCTL_BROWSER_CONTEXT`；显式文件坏了**失败**而不是静默）。
- **表单提交与 CLI `fetch` 也注入浏览器上下文**（同 #183）：`SessionEngine::send` 改用 `inject_headers`（同一 SEC-06 跨源规则与去重优先级），四条取回路径行为一致。

### Changed

- **所有降级信封都带 `meta.actionable`**（[#124](https://github.com/Thatgfsj/webctl/issues/124)）：原因码表集中到 `webctl_core::actionable`，由 `Envelope::degraded` 统一补（覆盖 LIMIT 截断、`extract` max-results、会话分页、文档/PDF、表格跨行列裁剪等）；`next_steps` 为空数组表示"确实没有别的通道"，不许编造。新增扫描用例防止"新增降级路径却忘登记"。

### Fixed

- `doc_text` / `json_query` / `download` 在 JS 空壳页上**静默返回 OK**（[#125](https://github.com/Thatgfsj/webctl/issues/125)，[#182](https://github.com/Thatgfsj/webctl/pull/182)）：现在继承上游的 `DEGRADED` + `degraded_from=js-render` + `meta.actionable`，只继承 OK 结果（本工具自己的失败不被伪装成降级）。

### Notes

- **发布**：本条目即 **v0.2.1** 的发布说明（tag `v0.2.1`，commit `4d8ac06`；5 目标 / 10 压缩包 / 28 资产，Linux 另含 musl 静态）。资产清单与校验和见 Release 页与 `sha256.sum`；体积与冷启动数字见 [RELEASE §7](../docs/RELEASE.md)。

## [0.2.0] - 2026-10-25

本版主题：**站点定位兜底**（用户说得出站点名、给不出 URL 时给出带证据的候选）
与**浏览器上下文注入**（借用宿主/设备浏览器的 HTTP 头与 Cookie）。决策依据：ADR-0006。

### Added

- **第 17 个工具 `resolve_site`**（[#173](https://github.com/Thatgfsj/webctl/pull/173)）：多候选 + 证据 + 置信度，**绝不单选**。
  通道阶梯 L1–L5：
  - **L1** 内置映射表（36 条）+ 域名直推（纯本地、不联网）；
  - **L2** 站点内建搜索（模板表 24 条，每次最多 2 个站点；**robots 拦下不请求**；分值 55/58，**永不跨可靠阈值**）（[#178](https://github.com/Thatgfsj/webctl/pull/178)）；
  - **L3** Wikidata P856 官方网站（免 key 只读 API；两步请求都过 politeness）（[#175](https://github.com/Thatgfsj/webctl/pull/175)）；
  - **L4** 候选域名 robots.txt 的 `Sitemap:` 声明（**只报告声明**，不提升置信度）（[#176](https://github.com/Thatgfsj/webctl/pull/176)）；
  - **L5** 显式 opt-in 的 SERP 兜底（`allow_serp_fallback` **默认 false**、结果低分、被拦即 `BLOCKED`）（[#177](https://github.com/Thatgfsj/webctl/pull/177)）。
- **浏览器上下文注入**系列（[#161](https://github.com/Thatgfsj/webctl/pull/161)…[#169](https://github.com/Thatgfsj/webctl/pull/169)）：`WEBCTL_BROWSER_CONTEXT` 指向的 JSON 文件；
  `user_agent` / `accept` / `accept_language` / `sec_ch_ua` / `referer` / `cookies[]`；凭据类头**只同源携带**；
  cookie 值**不落盘/不落日志/不进 envelope**（`BrowserContext` 刻意不实现 `Serialize`）；TLS/JS 指纹不可借用（诚实边界）。
- **robots.txt 支持 `Sitemap:` 解析**（`RobotsRules::sitemaps()`）。

### Changed

- **CI 政策（用户要求）**（[#166](https://github.com/Thatgfsj/webctl/pull/166)）：**PR 只跑 `static` + `guards`**；三平台矩阵与 musl 静态改到 **push main / 手动 / 发布**；
  分支保护的必需检查同步收敛为 2 项。覆盖面变化如实登记在 `docs/CI.md §8`（合并红线 9 → 2）。
- 大文件拆分（[#171](https://github.com/Thatgfsj/webctl/issues/171)）：`tests/mcp.rs`、`session/form.rs`、`tests/session.rs` 均拆到 < 1000 行，全仓无超限文件。

### Notes

- **发布**：本条目即 **v0.2.0** 的发布说明；资产清单与校验和见 Release 页（tag `v0.2.0`）与 `docs/RELEASE.md`。若资产有问题，按 `docs/RELEASE.md §3` 的回滚步骤处理。

## [0.1.2] - 2026-10-19

本版主题：**JS 密集型 / SPA 站点的有效信息获取**（EPIC #113）。
三条路线同时推进：内嵌结构化数据、可行动降级、以及受限的内置 JS 引擎。

### Added

- **内嵌结构化数据通道**：新工具 `embedded_data`（工具面第 16 个），从 JSON-LD、`script[type=json]`（如 `__NUXT_DATA__`）、框架状态容器（`inline-state`，如 `window.__NUXT__`）与 `data-page`（Inertia）里取结构化数据（[#114](https://github.com/Thatgfsj/webctl/issues/114)）。
- **降级带可行动信息**：`meta.actionable = {reason, detected, next_steps}`，让调用方知道"为什么降级、看到了什么、还能试什么"；`caps` 同时暴露 SPA 支持面（[#115](https://github.com/Thatgfsj/webctl/issues/115)）。
- **SPA 样本集与「有效信息获取率」基线**：`docs/SPA-BASELINE.md` + 可执行的离线基线测试（[#119](https://github.com/Thatgfsj/webctl/issues/119)）。
- **受限内置 JS 引擎**（[ADR-0005](docs/adr/0005-embedded-js-engine.md)）：`crates/webctl-js` 用纯 Rust 的 Boa 执行页面**内联**脚本，配套最小 DOM/BOM 垫片与 `render()`；
  沙箱**零宿主能力**（无网络 / 文件 / 子进程 / 定时器，动态 `import` fail-closed），限值覆盖 loop / recursion / stack 与输入预算。
- **`js_render` 开关（默认关闭）** 与引擎接线：开启后 `extract` 等工具先渲染再解析；`caps.js` 如实区分 `enabled`（调用方是否开启）与 `wired`（是否真的接线）。
- **CI 重写**：三平台矩阵（ubuntu / macos / windows）、合并安全守卫（全量 `--locked`、每作业超时、并发取消、最小权限、`#[ignore]` 零容忍、工作区洁净断言）、
  失败精准定位为 **`file:line:col` 的 GitHub 注解 + 作业摘要**；配套 `docs/CI.md` 与 `docs/TESTING.md §1` 的通道映射。

### Changed

- **CI 性能**：`guards` 作业加 Rust 缓存（热运行 146s → 43s）；release 构建（L2）只在 Linux 上跑（Windows 367s → 251s）；日志仅在失败/取消时上传。
  总墙钟热缓存 **373s → 257s（−31%）**；覆盖面变化（PR CI 不再验证 macOS/Windows 的 release profile 编译）已如实登记在 `docs/CI.md §7.3`。

### Fixed

- **安全**：修掉沙箱的一处真实逃逸——Boa 默认模块加载器以**进程 CWD** 为根，页面脚本可用相对 `import()` 真实读取宿主文件（[#127](https://github.com/Thatgfsj/webctl/issues/127)）。
> 已知**尚未进入本版**的改动：`doc_text` / `json_query` / `download` 在 JS 页的降级继承（[#125](https://github.com/Thatgfsj/webctl/issues/125)）——旧分支 #139 已关闭，改由 [#182](https://github.com/Thatgfsj/webctl/pull/182) 在 0.2.0 之后落地，见下方 [0.2.1] 段的 Fixed 项。

- 最小 DOM：解析器补 RAWTEXT（`script`/`style`/`textarea`/`title` 按原文处理）、序列化函数防止被页面改写、垫片避开 Annex B 的 `substr`。
- CI：`dist` 安装方式修正（`cargo-dist` 已更名 `dist`，crates.io 上无 0.33.0；改用官方脚本 + sha256 固定 + 版本断言），修掉 main 上长期红灯的 `static` 作业（[#140](https://github.com/Thatgfsj/webctl/pull/140)）。

### 达成与未达成（按 PLAN §5.3 的口径，逐条分清）

- **验收锚点已达成**（PLAN §5.3 第 1 条：该页必须 `DEGRADED` 并说明原因，**不得**「成功但空」）：`quotes.toscrape.com/js/` 返回 `DEGRADED` + `degraded_from` + `meta.actionable` ✓。
- **能力目标只对内联脚本页达成**：开启引擎后该页**仍取不到内容** ✗，原因是它的内联脚本依赖**外链** jQuery
  （`ReferenceError: $ is not defined`）。本版让"内容由**内联**脚本生成"的页面可取，让"依赖外部 JS 库"的页面**如实降级**。
  是否执行同源外链脚本见 [#137](https://github.com/Thatgfsj/webctl/issues/137)。
- 引擎限值只有 loop / recursion / stack 与输入预算；**堆内存与墙钟不设限**（Boa 0.20 不提供这些 API），归属决策见 [#130](https://github.com/Thatgfsj/webctl/issues/130)。
- musl 静态体积基线待用**真实发布资产**回填（本机无 Bootlin 工具链，未编造数字）。

## [0.1.1] - 2026-10-17

### Added

- CI 增加 `musl-static` 作业：每次 PR 用固定版本 Bootlin 2026.08-1 musl C++ 工具链
  （GCC 15.3.0 / musl 1.2.6，SHA256 **每次运行都校验**）构建 `webctl`/`webctl-mcp`，
  并用 `file`/`ldd`/`readelf` 断言静态（[#90](https://github.com/Thatgfsj/webctl/issues/90)）。
- `x86_64-unknown-linux-musl` 恢复到 cargo-dist 发布矩阵，本版起产出 musl 静态资产。
- **L4 性能基线通道**（[#83](https://github.com/Thatgfsj/webctl/issues/83)）：
  `crates/webctl-web/examples/perf_probe.rs`（确定性自生成输入）与 `scripts/perf-baseline.sh`
  （相对基线 + 门槛，`--record`/`--check`），覆盖单测 30s 挂死守卫抓不到的 `[~5s, 30s)` 档。
- **CI 行数门禁**（[#41](https://github.com/Thatgfsj/webctl/issues/41) / [#24](https://github.com/Thatgfsj/webctl/issues/24)）：
  `scripts/pr-size.sh` 作为唯一口径实现，`pr-size` 作业在计入 >1000 行时失败，
  维护者批准后打 `lines: exempt` 标签放行。
- `docs/DSH-INTEGRATION.md`（[#28](https://github.com/Thatgfsj/webctl/issues/28)）：
  DSH MCP 的 7 条硬约束逐条核对（指令字节数、探测/并发启动、`toolOrder`、预算、会话归属）。
- `docs/MIGRATION.md` 补记旧仓处置：README 顶部迁移提示 + 归档（[#26](https://github.com/Thatgfsj/webctl/issues/26)）。

### Changed

- MCP 侧 JS 降级传播接线并收口 SEC-09（[#82](https://github.com/Thatgfsj/webctl/issues/82)）：
  `search`/`links`/`tables`/`forms` 的零结果接入与 CLI/库同一套降级判定；删除 PR 11 留的
  交接常量与 `#[ignore]` 哨兵；新增跨路径一致性用例与 MCP 侧脱敏的**隔离**用例。
- README（中英）、RELEASE、ACCEPTANCE、TESTING 与体积基线同步为如实口径；
  「调用预算 60 秒（MCP-09）/ 请求预算 30 秒（`timeout_ms`）」两处措辞与代码对齐。

### Fixed

- `scripts/install-musl-toolchain.sh` 改为**每次运行都校验**工具链 SHA256（原先缓存命中时整段跳过，
  与脚本头"每次校验"的声称矛盾），并写明该工具链属 TOFU 信任（无 GPG、不从源码重建）。
- 测试的 `#[ignore]` 由 3 条降为 **0 条**：DOC-12 改为真实调用 `doc_text`/`json_query`；
  与 `tests/mcp14.rs` 重复的 MCP-14 占位删除。

### 已知限制

- **未在真实 DSH 里挂载并调用 `mcp__webctl__*`**（`docs/DSH-INTEGRATION.md` §5 已登记）；
- **未做 Alpine/容器干净环境 E2E**（构建机无 Docker/podman）；静态性证据为 `ldd` + `readelf -d` 无 NEEDED + `env -i` 冒烟；
- `aarch64-unknown-linux-musl` 未纳入发布矩阵；
- L4 性能通道是**手动**通道（发布前必跑），对机器负载敏感（有效门槛见 `docs/TESTING.md` §6.11）。
## [0.1.0] - 2026-10-17

### Added

- 打包与发布流水线：cargo-dist 配置（`dist-workspace.toml`）与自动生成的
  `.github/workflows/release.yml`；三平台资产（Linux / macOS / Windows）；**musl 静态目标暂缓**（见下方「已知限制」与 [#90](https://github.com/Thatgfsj/webctl/issues/90)）。
- 体积与冷启动基线工具 `scripts/size-baseline.sh` 及基线记录 `docs/size-baseline.json`（PKG-05）。
- 发布资产 E2E 冒烟脚本 `scripts/release-e2e.sh`（PKG-04 / PKG-06 / PKG-08）。
- 双语 README（中文为主 + 英文摘要）与 [docs/RELEASE.md](docs/RELEASE.md) 发布手册（PKG-07）。

### 已知限制

- **musl 静态目标暂缓**：BoringSSL（C++）需要面向 musl 的 libstdc++，而 Debian/Ubuntu 的 `musl-dev` 只提供 C 侧；
  三次真实 release 均失败（缺交叉 C++ 编译器 → 链接用宿主 cc → `fatal error: memory: No such file`）。
  因此 Linux 发布物为 **glibc 动态链接**；静态目标见议题 [#90](https://github.com/Thatgfsj/webctl/issues/90)。
> v0.1.0 已于 2026-10-17 发布：[https://github.com/Thatgfsj/webctl/releases/tag/v0.1.0](https://github.com/Thatgfsj/webctl/releases/tag/v0.1.0)。三平台资产（`webctl` 与 `webctl-mcp`）+ shell/PowerShell 安装器；Linux 为 glibc 动态链接。
