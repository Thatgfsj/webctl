# 变更日志

本文件遵循 [Keep a Changelog 1.1.0](https://keepachangelog.com/zh-CN/1.1.0/) 与
[语义化版本](https://semver.org/lang/zh-CN/)。格式约定、发版步骤与 release note 模板见
[docs/RELEASE.md](docs/RELEASE.md)。

## [0.1.2] - 2026-10-19

本版主题：**JS 密集型 / SPA 站点的有效信息获取**（EPIC #113）。
三条路线同时推进：内嵌结构化数据、可行动降级、以及受限的内置 JS 引擎。

### Added

- **内嵌结构化数据通道**：新工具 `embedded_data`（工具面第 16 个），从 JSON-LD / Microdata / `__NUXT_DATA__` 一类内联状态里取结构化数据（[#114](https://github.com/Thatgfsj/webctl/issues/114)）。
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
> 已知仍在评审中、**尚未进入本版**的改动：`doc_text` / `json_query` / `download` 在 JS 页的降级继承（[#125](https://github.com/Thatgfsj/webctl/issues/125)，PR #139 待合）。

- 最小 DOM：解析器补 RAWTEXT（`script`/`style`/`textarea`/`title` 按原文处理）、序列化函数防止被页面改写、垫片避开 Annex B 的 `substr`。
- CI：`dist` 安装方式修正（`cargo-dist` 已更名 `dist`，crates.io 上无 0.33.0；改用官方脚本 + sha256 固定 + 版本断言），修掉 main 上长期红灯的 `static` 作业（[#140](https://github.com/Thatgfsj/webctl/pull/140)）。

### 如实登记的未达成项（EPIC #113）

- **核心锚点未达成**：`quotes.toscrape.com/js/` 在开启引擎后**仍取不到内容** ✗，原因是该页的内联脚本依赖**外链** jQuery
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
