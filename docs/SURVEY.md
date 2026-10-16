# webctl 调查证据

本文件是 [PLAN.md](PLAN.md) 的全部事实依据。**凡标注「估算」的数字未实测**；其余均为本机命令或公开仓库/API 的实测结果，附来源。

调查日期：2026-10-16

---

## 1. 项目规范调查

### 1.1 dsh-ops 的成文规范（原文）

来源：[T-Auto/dsh-ops `docs/CONTRIBUTING.md`](https://github.com/T-Auto/dsh-ops/blob/HEAD/docs/CONTRIBUTING.md)

> - dsh-ops 专注践行极简主义，低侵入性，仅为 dsh 提供额外的易用 tools……对原版的逻辑更改仅限于可选的禁用原版重复工具，以保证和其他插件的兼容性
> - **pr 必须关联 issue，且主动在 issue 领取 —— 否则不通过**，这是为了以 issue 是否被领取来确认彼此的工作，防止重复
> - issue 是 pr 工作区，非反馈 bug 或者写新功能的讨论，请移步至 Discussions
> - **feat 前请先写 issue，经由维护团队批准后再写 feat 功能**。dsh-ops 团队希望保持代码和功能简洁
> - dsh-ops 会和一些 dsh 插件联动。**对插件间联动层的实现单开逻辑区域，要完全和核心逻辑解耦**

补充事实：PR [#102](https://github.com/T-Auto/dsh-ops/pull/102)（标题为 "docs: document the local gate and add pull request and issue templates"）**未合并**（`mergedAt: null`），仓库中**没有 PR 模板文件**；规范只存在于 CONTRIBUTING。

### 1.2 「每 1000 行 PR 需审批」的核实结果：**仓库中不存在**

做了反向核实（本地官方克隆 + T-Auto 6 个仓库 clone 到 `/tmp/tauto` + 用户账号与 4 个组织 + GitHub 全站代码搜索）：

- `grep -rn "1000"` 在 `.scratch/dsh-src` 命中约 30 处，**全部无关**（`pasteReadTimeoutMs` 默认 1000ms、`maxEntries` 默认 1000、分页 `limit=1000`、`maxDiffEditLength` 默认 1000 等）。
- T-Auto 6 仓库的尺寸类 grep（`1000 (行|line|lines|loc)`）→ **零命中**；`模块化|modulariz` 只命中 `awesome-dsh-plugins` 的插件描述文本。
- `gh search code "1000 lines" --owner T-Auto`、`"1000 行"`、`"1000 lines approval"`、`"PR 1000 行"` → **全部为空**。
- `Thatgfsj` / `Thatgfsj-easyRep` / `Thatgfsj-contribs` / `XXGCIELab` 的 `.github` 仓库仅有 README，**无 CONTRIBUTING、无 PR 模板、无 CODEOWNERS、无 workflow**。
- 官方克隆中**无 CODEOWNERS 文件**（`find . -iname CODEOWNERS` 为空）。

**反证（证明是"不存在"而非"没找到"）**：dsh-ops 有大面积超千行 PR 正常合并 —— `#110`（+5864/−496）、`#81`（+5347/−245）、`#121`（+754/−174005）、`#103`（+3073，仍 open）。若存在 1000 行审批线，这些无法无摩擦落地。

**T-Auto 真实存在的门禁是"加权 2 分制"，与行数无关**（`T-Auto/.github` → `.github/review-ownership/`）：

| 机制 | 数值 |
|---|---|
| 通过阈值 | `requiredPoints: 2` |
| 评审人权重 | 默认 1 分；白名单 6 人（`07akioni`、`imccyu`、`tianyicui`、`tianyicui-bot`、`turtle1999`、`turtle2099`）各 2 分 |
| 归权公式（**唯一按行处**） | `min(2, 1 + 4 × ownedLines / totalLines)`；归属 25% 即可单人满足 2 分 |
| 作者信用（**唯一 `/1000`**） | `min(100, mergedPRCount) × 11 / 1000` 分，上限 1.1，且"不能单独满足 2 分" |
| 委派 | `/delegate @username`，每位发送者至多一次 |
| 策略生效 | 必须已合并进默认分支；**在 PR 内改门禁无效** |
| 阻塞条件 | draft 或有效 `CHANGES_REQUESTED` |

另两处易混的「1000」：DSH 的 fine-grained approval **每 1000 个输出条目**通知一次（`itemIndex % 1000 === 0`）；`awesome-dsh-plugins/CONTRIBUTING.md` 里的 1000 是 **GitHub 搜索 API 结果上限**。三者均与 PR 代码行数无关。

**分布实证（不是规则依据，仅供参考）**：dsh-ops 54 个 PR 的改动行数中位数 **262**；900–1000 区间有 `#79`=904、`#2`=966；超过 1000 行的 7 个均为大特性 / 平台移植 / 发布 / 大重构。

**结论**：「每 1000 行代码的 PR 需要审批」属于**尚未写下的新规则**。本计划按维护者指令执行它，并在 PLAN §6.2 给出可机械校验的措辞（口径、排除项、批准方式）。

### 1.2.1 真实存在的「PR 粒度 + 模块化」条款（应遵守）

| # | 条款（原文摘要） | 出处 |
|---|---|---|
| B1 | 「Choose PR history deliberately. **Split independent changes** and fix the introducing PR before propagation」 | DSH `AGENTS.md` |
| B2 | ≥2 层依赖的 PR 链必须用 GitHub 官方 stack；部分落地需显式边界 PR | DSH `.agents/notes/implemented/process/2026-08-02-native-github-stacks-and-optional-rebases.md` |
| B3 | 「一次 PR 不应同时」做多类变更（挂载 / `registry/` 索引 / `docs/` / revision 升级 / 治理与 CI 规则），禁止"顺手规范化" | `dsh-ecosystem-spec/CONTRIBUTING.md` §4 |
| B4 | 生态收录「**一次 PR 只加一类**」 | 同上，表格第 2 行 |
| B5 | 挂载 revision 升级必须是**独立 PR** | `dsh-ecosystem-spec/governance/rules.md` §3 |
| B6 | 「对插件间联动层的实现**单开逻辑区域，要完全和核心逻辑解耦**」 | `dsh-ops/docs/CONTRIBUTING.md` 第 5 条 |
| B7 | 「pr 必须关联 issue，且主动在 issue 领取 —— 否则不通过」；「feat 前请先写 issue，经由维护团队批准后再写 feat 功能」 | 同上，第 2–4 条 |
| B8 | capability seam「split only when roles evolve independently」；「**Don't split preemptively**」 | DSH `AGENTS.md` + capability-seams 决策记录 |
| B9 | 「Keep business rules in their own package」 | DSH `packages/AGENTS.md` |
| B10 | 「Plugins, not loop changes」：新行为走既定扩展点 | DSH `AGENTS.md` |
| B11 | 「Do not turn an implementation convenience, current repository layout, or one product's limitation into a normative requirement.」 | `dsh-std/AGENTS.md`；同义句见 `dsh-distribution/CONTRIBUTING.md` |

**关键含义**：真实规范管的是**变更类别与依赖方向**，不是行数。因此 PLAN §4 采用「角色不独立演进就不预先拆包」的 7-crate 布局，把 1000 行约束落在**目录与 PR 边界**上；同时 B11 提示：新增的 1000 行规则必须写成"评审可复核性"要求，而不是把仓库布局当成规范。

### 1.3 浏览器 profile 限制（影响登录态复用方案）

Chrome/Edge **136 起，默认 user profile 不允许开启 remote-debugging**，必须显式指定 `--user-data-dir`。
来源：[Microsoft Q&A — Edge Remote Debugging is Not Working When Using the Default User Profile](https://learn.microsoft.com/en-nz/answers/questions/5976829/microsoft-edge-remote-debugging-is-not-working-whe)

→ 决策：只支持受管 profile；登录态复用改由 `profile import`（复制 cookie）实现。

### 1.4 DSH / MCP 集成约束（原文要点）

来源：本地 `AIagent/.scratch/dsh-src/packages/mcp/mcp-client/README.md`

- stdio 配置字段：`serverName` / `transport: stdio` / `command` / `args` / `env` / `cwd`；另有 `toolCallTimeoutMs`（默认 **60,000ms**）、`maxInstructionBytes`（默认 32,768）、`failOnStartupError`、`reconnect.*`。
- 工具名形如 `mcp__<serverName>__<rawName>`，需满足 DeepSeek function-name 契约。
- **stdio 协商会先启动一个临时探测进程，然后才启动服务进程** → 服务端必须能被启动两次而不冲突。
- 连接失败时 harness 仍会启动，但该服务器的工具不出现。

---

## 2. 本机实验（可复现）

### 2.1 环境

| 项 | 值 |
|---|---|
| CPU / 内存 / 磁盘 | 16 核 / 15GiB / 276G 可用 |
| Rust | `rustc 1.98.1`，`cargo 1.98.1` |
| cargo 源 | rsproxy-sparse 镜像（`~/.cargo/config.toml`） |
| 浏览器 | **仅 `microsoft-edge` 155.0.4283.45**（无 Chrome/Chromium） |
| 网络 | 走本机代理；crates.io 与 static.crates.io 均 200 |
| 可用 CLI | `curl` `wget` `lynx` `jq` `pdftotext` `pdfinfo` `unzip` ✅；`w3m` `pandoc` `htmlq` `xmllint` `tesseract` `rg` ❌ |

### 2.2 手写 CDP 可行性（关键实验）

脚本：`.scratch/webctl-probe/run-cdp-probe.sh` + `.scratch/webctl-probe/cdp-probe.mjs`（**未使用 Playwright**，纯手写 CDP 帧）。

| 验证项 | 结果 |
|---|---|
| 协议版本 | `Protocol-Version: 1.3`，浏览器 `Edg/155.0.4283.45` |
| 目标与会话 | `Target.createTarget` ✅、`Target.attachToTarget(flatten)` ✅ |
| 域启用 | `Page` / `Runtime` / `Network` / `DOM` / `Accessibility` 全部 ✅ |
| 导航 | `Page.navigate` → frameId ✅ |
| 真实点击 | `DOM.getBoxModel` 算坐标 + `Input.dispatchMouseEvent` → 页面文本变为 `已点击1` ✅ |
| 中文输入 | `Input.insertText` → `中文输入测试` ✅（证实不能走 `dispatchKeyEvent`） |
| 可访问性树 | `Accessibility.getFullAXTree` 24 节点；`aria-label="提交按钮"` 取到 name ✅ |
| 网络事件 | 刷新后收到 18 条 `Network.*` 事件 ✅ |
| 截图 | `Page.captureScreenshot` → 7KB PNG ✅ |
| 反检测 | `navigator.webdriver === false` ✅ |

### 2.3 时延实测

脚本：`.scratch/webctl-probe/timing.sh`

| 轮次 | Edge 启动 → CDP 就绪 | 关闭耗时 |
|---|---|---|
| 1 | 305 ms | 104 ms |
| 2 | 292 ms | 105 ms |
| 3 | 313 ms | 104 ms |

→ 浏览器按会话懒启动的开销可接受；「冷启动 < 50ms」的旧假设不成立（那是纯 Rust 路径的指标，不是浏览器路径）。

### 2.4 无浏览器兜底能力与 T1 的真实边界

| 实验 | 结果 |
|---|---|
| `curl -sL https://example.com \| lynx -dump -stdin` | 可读文本 ✅（T2 兜底成立） |
| `quotes.toscrape.com/js/`（JS 渲染）静态抓取 | 匹配 `class="quote"` = **0 条** |
| `quotes.toscrape.com/`（静态版） | **10 条** |

→ 结论：无浏览器时 T1 只覆盖服务端渲染/表单流；SPA 必须显式降级。
→ 同时这条对比是 T3 的**验收锚点**：静态 0 条，webctl 必须拿到 10 条。

### 2.5 依赖可达性（rsproxy 镜像实测）

`cargo add <crate> --dry-run` 全部解析成功：

| crate | 解析版本 | crate | 解析版本 |
|---|---|---|---|
| tokio-tungstenite | 0.30.0 | html2text | 0.17.3 |
| reqwest | 0.13.5 | clap | 4.6.7 |
| scraper | 0.27.0 | rmcp | 3.5.1 |
| dom_smoothie | 0.18.2 | | |

### 2.6 移动仿真陷阱（实测）

`Emulation.setDeviceMetricsOverride({width:390,height:844,mobile:true})` 之后，页面 `window.innerWidth` 仍为 **980**——因为页面没有 `<meta name="viewport">`。
→ 移动仿真需要「设备指标 + viewport meta + touch 仿真 + UA 覆盖」多件套，不能只设宽高。

---

## 3. Prior art 硬数字（源码 `wc -l` 实测）

### 3.1 CDP 协议规模（两侧一致）

| 指标 | 数值 | 来源 |
|---|---|---|
| browser domains / js domains | 47 / 6 | `headless_chrome-1.0.22/json/*.json` |
| command 总数 | **641**（browser 560 + js 81） | 同上 |
| event 总数 | 216（browser 194 + js 22） | 同上 |
| object 类型 | 381 | 同上 |
| enum 类型 / variant | 158 / 1,408 | 同上 |
| 字段总数（optional） | 3,847（1,456） | 同上 |

### 3.2 chromiumoxide 0.9.1 —— 协议独立 crate + 离线 PDL 生成

手写 `src/` 合计 **9,938 行**：

| 文件 | LOC | 层次 |
|---|---|---|
| `conn.rs` | **160** | 传输层（async-tungstenite Stream impl） |
| `cmd.rs` / `error.rs` | 167 / 156 | 序列化 / 错误 |
| `listeners.rs` | 294 | 事件分发 |
| `handler/`（15 文件） | **3,539** | 会话语义状态机（target.rs 784、mod.rs 766、frame.rs 696、page.rs 465、network.rs 289…） |
| `page.rs` | 1,517 | 高阶 API |
| `element.rs` / `layout.rs` | 504 / 297 | 元素与几何 |
| `keys.rs` | 1,493 | 键盘映射（纯数据表） |
| `browser/{mod,config,argument}.rs` | 581 / 488 / 134 | 启动配置 |
| `async_process.rs` + `detection.rs` | 140 + 163 | 进程与探测 |

协议类型（**不在主 crate 内**）：

- `chromiumoxide_cdp-0.9.1/src/cdp.rs` = **111,037 行 / 5,189,141 字节**，单文件，首行自述 "This file is generated and should not be edited directly."
  - 分段：文件头 + trait 118 → `pub mod events` 7,979 → `pub mod js_protocol` 11,020 → `pub mod browser_protocol` 91,920
  - 单 domain 实测 LOC：`network` **11,795**、`page` **9,649**、`css` 7,722、`storage` 6,801、`dom` 6,479、`audits` 5,354、`emulation` 4,389、`overlay` 3,464
  - 计数：2,936 `pub struct`、237 `pub enum`、6,696 `pub fn`、8,059 `impl`
- `chromiumoxide_pdl-0.9.1` = **3,097 行**（generator 1,224 / parser 450…），输入 **16,525 行 PDL**（Network.pdl 2,340、Page.pdl 1,784、CSS.pdl 1,020）
- `chromiumoxide_types-0.9.1/src/lib.rs` = **335 行**（线协议信封）

### 3.3 headless_chrome 1.0.22 —— build.rs 代码生成

手写 `src/` 合计 **7,896 行**：`tab/mod.rs` 1,939、`tab/keys.rs` 1,549、`browser/process.rs` 751、`tab/element/mod.rs` 618、`browser/mod.rs` 617、`browser/fetcher.rs` 466、`types.rs` 377、`transport/mod.rs` 361、`transport/web_socket_connection.rs` 306、`transport/waiting_call_registry.rs` **115**、`element/box_model.rs` 198。

生成链路：`build.rs` **6 行** → `auto_generate_cdp 0.4.6`（**1,770 行**）→ `src/protocol.rs` 仅 1 行 `include!(...)`。输入为 vendored JSON（29,464 + 3,800 行）。
实测重建产物：**908,588 字节、2,082 `pub struct`、234 `pub enum`、641 `impl Method for`、53 个 domain 模块**；格式化后行数未实测（本机无 rustfmt 组件），**估算 6–9 万行**。

### 3.4 rmcp 3.5.1 —— 最小 stdio server（实测编译 + 握手通过）

- features：`server`（默认）+ `macros`（默认）+ `schemars` + **`transport-io`（不在默认 features，必须显式开启）**。
- 骨架 **25 行**；工具用宏声明（`#[tool_router]` / `#[tool(description=…)]` / `#[tool_handler]`），参数用 `Parameters<T>`（`T: schemars::JsonSchema`），每个工具约 **10–20 行**。
- 实测：`cargo build` 20.26s 成功；管道喂入 `initialize` → 返回 `protocolVersion 2024-11-05`；`tools/list` 返回正确 `inputSchema`。
- 官方 example（**发布包内无 examples 目录，需从仓库取**）：`examples/servers/src/calculator_stdio.rs` 26 行、`common/calculator.rs` 34 行 / 2 个工具、`memory_stdio.rs` 18 行。仓库：<https://github.com/modelcontextprotocol/rust-sdk>

### 3.5 工程教训（直接来自上述实测）

1. **协议类型 111,037 行，必须独立 crate 且排除在 1000 行 PR 预算之外**——这是整个拆分方案的前提。
2. **传输层极薄（160 行）**，自研 WebSocket 完全不划算，直接依赖 tungstenite 系。
3. **真正的复杂度在"命令 id 关联 + 会话路由 + 导航/框架状态机"**：`handler/` 3,539 行 vs `conn.rs` 160 行；而"纯 id 关联"可以小到 115 行（`waiting_call_registry.rs`）。→ 分发（纯关联）与 target/frame 语义必须拆开。
4. **单个 domain 的生成代码就超 1000 行**（network 11,795 / page 9,649）→ 不要手写全量协议类型。
5. **离线生成 + 提交产物优于 build.rs 生成**（可评审、可锁版本；build.rs 方案把协议版本钉死在生成时的 commit）。
6. **MCP 层不会成为瓶颈**（骨架 25 行 + 每工具 10–20 行）；瓶颈只在协议类型与 page/target 语义两处。

### 3.6 汇总估算

手写部分约 **3,000–7,000 行**（不含协议类型），可切 10–15 个 PR；协议类型若手写最小子集约 **800–1,200 行（估算）**，作为独立 crate 交付。

---

## 4. 优秀项目清单（全部实测）

数据获取：`gh api repos/<r>` 与 crates.io API，2026-10-16。

| 项目 | ⭐ | 最近推送 | 许可证 | 语言 | 结论 |
|---|---|---|---|---|---|
| [lightpanda-io/browser](https://github.com/lightpanda-io/browser) | 36581 | 2026-10-16 | **AGPL-3.0** | Zig | ⚠️ 候选后端：非 Chromium 的"给机器用的浏览器"。独立进程 + CDP 通信，许可与 CDP 覆盖度**待正式确认** |
| [browser-use/browser-use](https://github.com/browser-use/browser-use) | 118175 | 2026-10-16 | MIT | Python | 只借鉴动作语义 |
| [ChromeDevTools/chrome-devtools-mcp](https://github.com/ChromeDevTools/chrome-devtools-mcp) | 53657 | 2026-10-16 | Apache-2.0 | TS | 只借鉴工具面 |
| [microsoft/playwright-mcp](https://github.com/microsoft/playwright-mcp) | 38230 | 2026-10-08 | Apache-2.0 | TS | 只借鉴快照/ref 方案 |
| [browserbase/stagehand](https://github.com/browserbase/stagehand) | 25697 | 2026-10-16 | MIT | TS | 借鉴 observe/extract 语义 |
| [Skyvern-AI/skyvern](https://github.com/Skyvern-AI/skyvern) | 23218 | 2026-10-16 | AGPL-3.0 | Python | 只读参考 |
| [seleniumbase/SeleniumBase](https://github.com/seleniumbase/SeleniumBase) | 13079 | 2026-10-13 | MIT | Python | 反检测思路 |
| [g1879/DrissionPage](https://github.com/g1879/DrissionPage) | 12517 | 2026-10-14 | NOASSERTION | Python | 反检测思路 |
| [daijro/camoufox](https://github.com/daijro/camoufox) | 12582 | 2026-10-15 | MPL-2.0 | Python | 反检测思路 |
| [steel-dev/steel-browser](https://github.com/steel-dev/steel-browser) | 7821 | 2026-10-13 | Apache-2.0 | TS | 会话/远程浏览器架构 |
| [ultrafunkamsterdam/nodriver](https://github.com/ultrafunkamsterdam/nodriver) | 4848 | 2026-10-08 | AGPL-3.0 | Python | 反检测思路 |
| [seanmonstar/reqwest](https://github.com/seanmonstar/reqwest) | 11874 | 2026-10-02 | Apache-2.0 | Rust | ✅ 直接依赖 |
| [modelcontextprotocol/rust-sdk](https://github.com/modelcontextprotocol/rust-sdk) | 4021 | 2026-10-15 | — | Rust | ✅ 直接依赖（MCP） |
| [spider-rs/spider](https://github.com/spider-rs/spider) | 2783 | 2026-10-01 | MIT | Rust | 参考并发抓取 |
| [rust-scraper/scraper](https://github.com/rust-scraper/scraper) | 2424 | 2026-10-05 | ISC | Rust | ✅ 直接依赖 |
| [rust-headless-chrome](https://github.com/rust-headless-chrome/rust-headless-chrome) | 2952 | 2026-06-11 | MIT | Rust | 借鉴协议组织方式 |
| [jonhoo/fantoccini](https://github.com/jonhoo/fantoccini) | 2027 | 2026-10-01 | Apache-2.0 | Rust | ❌ 需 driver 进程 |
| [stevepryde/thirtyfour](https://github.com/stevepryde/thirtyfour) | 1435 | 2026-10-12 | NOASSERTION | Rust | ❌ 需 driver 进程 |
| [mattsse/chromiumoxide](https://github.com/mattsse/chromiumoxide) | 1410 | **2026-04-03** | Apache-2.0 | Rust | 借鉴架构；**半年未动**，不做依赖 |
| [0x676e67/wreq](https://github.com/0x676e67/wreq) | 1068 | 2026-10-15 | Apache-2.0 | Rust | ✅ 直接依赖（TLS 指纹） |
| [jugglerchris/rust-html2text](https://github.com/jugglerchris/rust-html2text) | 245 | 2026-10-12 | MIT | Rust | ✅ 直接依赖 |
| [niklak/dom_smoothie](https://github.com/niklak/dom_smoothie) | 226 | 2026-10-14 | MIT | Rust | ✅ 直接依赖 |

### 4.1 选型结论

**依赖 8 个成熟 crate，借鉴 4 个 AI 浏览器工具的工具面，自研范围收敛为"CDP 最小客户端 + 分层路由 + 交互原语"。**

---

## 5. 模块化治理标杆（实测 crate 粒度）

| 项目 | ⭐ | `Cargo.toml` 数 | 贡献规范文件 |
|---|---|---|---|
| [astral-sh/uv](https://github.com/astral-sh/uv) | 90813 | 93 | `CONTRIBUTING.md` + `docs/reference/contributing.md` |
| [zed-industries/zed](https://github.com/zed-industries/zed) | 91810 | 267 | `CONTRIBUTING.md` |
| [denoland/deno](https://github.com/denoland/deno) | 108695 | 87 | `.github/CONTRIBUTING.md` |
| [BurntSushi/ripgrep](https://github.com/BurntSushi/ripgrep) | 69149 | 12 | `CONTRIBUTING.md` |
| [typst/typst](https://github.com/typst/typst) | 56656 | 22 | `CONTRIBUTING.md` |
| [astral-sh/ruff](https://github.com/astral-sh/ruff) | 50115 | 57 | `CONTRIBUTING.md` + **4 个 crate 级 CONTRIBUTING.md** |
| [swc-project/swc](https://github.com/swc-project/swc) | 34203 | 70 | `CONTRIBUTING.md` |
| [tokio-rs/tokio](https://github.com/tokio-rs/tokio) | 33430 | 13 | `CONTRIBUTING.md` + [`docs/contributing/pull-requests.md`、`reviewing-pull-requests.md`](https://github.com/tokio-rs/tokio/tree/HEAD/docs/contributing) |
| [biomejs/biome](https://github.com/biomejs/biome) | 25961 | 109 | `CONTRIBUTING.md` + **9 个 crate 级 CONTRIBUTING.md** |
| [oxc-project/oxc](https://github.com/oxc-project/oxc) | 23085 | 68 | `CONTRIBUTING.md` |
| [rolldown/rolldown](https://github.com/rolldown/rolldown) | 13973 | 54 | `CONTRIBUTING.md` |

**可照搬**：biome / ruff 的「**每个 crate 自带一份 CONTRIBUTING.md，声明本模块的边界与验收**」——这正是"模块化 + 每 1000 行 PR 审批"最契合的组织方式；tokio 的 `pull-requests.md` + `reviewing-pull-requests.md` 是 PR 粒度与评审流程的现成范本。

---

## 6. 数据来源与可信度

- **实测 LOC**：来自 `crates.io` 官方 tarball（`static.crates.io`）解包后 `wc -l`：`chromiumoxide-0.9.1`、`headless_chrome-1.0.22`、`rmcp-3.5.1`、`chromiumoxide_cdp-0.9.1`、`chromiumoxide_types-0.9.1`、`chromiumoxide_pdl-0.9.1`、`auto_generate_cdp-0.4.6`。
- **标注为「估算」**：headless_chrome 生成产物的格式化行数（6–9 万）、各模块 LOC 区间、按 domain 子集推导的协议类型行数。
- **本机实验**：脚本与产物在 `/home/thatgfsj/AIagent/.scratch/webctl-probe/`（`run-cdp-probe.sh`、`cdp-probe.mjs`、`timing.sh`），可原地重跑。
- **未修改任何仓库文件**；所有下载/解压/生成均在 `/tmp` 下完成。

---

## 7. 尚未完成 / 待补

| 项 | 状态 |
|---|---|
| 工具面定稿：chrome-devtools-mcp / playwright-mcp / browser-use / DSH browser-use 的完整工具清单与 ref 方案对比 | **已完成**，见 §8；结论已回填 PLAN §3.8 与 PR 13–17 |
| 「每 1000 行 PR 需审批」的成文条款 | **已完成**：不存在，详见 §1.2 / §1.2.1；作为待写入的新规则执行 |
| `Lightpanda` 的 CDP 覆盖度与 AGPL-3.0 许可结论 | 未开始（PLAN §9 待验证项） |
| `wreq` 在 Rust 1.98 + rustls 下的实际编译 | 未开始（P0 第一件事） |

---

## 8. 工具面调查：四家实现口径（工具面定稿依据）

版本口径：PW = `@playwright/mcp@0.0.83` + `playwright-core@1.64.0-alpha`；CDM = `chrome-devtools-mcp@1.11.0`；BU = `browser-use` 0.13.13（wheel 0.13.11，两者 `tools/service.py` 逐字节相同）；SH = `@browserbasehq/stagehand@4.2.1`（另对照 v3.7.3 / v2.5.9）；DSH = 本地 `.scratch/dsh-src`。
工具数量：**PW 72、CDM 67、BU 25（另有 MCP server 14）、SH v4 4 个原语、DSH 自家 6**。
取证产物：`/tmp/browserinv/`（含 `browser-use-report.md`、`stagehand-report.md`、`mine/pw-tools.txt`、`mine/cdm-tools-full.txt`）。未修改任何用户仓库文件。

### 8.1 必须对齐的核心集约 20 个

navigate / back、tabs、snapshot、**find**、click、hover、fill、**fill_form**、select_option、press_key、drag、upload_file、handle_dialog、scroll、wait_for、evaluate、screenshot、console_messages、network_requests、extract（+ search_page）。

关键事实：

- CDM 的 `fill_form` 描述明写「ALWAYS prefer this tool over multiple individual fill/click calls」→ 批量填表是一等公民，不是语法糖。
- PW 的 `browser_find(text|regex)` 官方描述明写 "cheaper than capturing the whole snapshot"，返回匹配节点 + 祖先路径 + 3 行上下文，**纯本地无 LLM** → 快照内搜索必须有。
- **PW 默认每个动作后自动附全量快照**（`response.ts:159-161`，`mode ?? 'full'`，daemon 硬编码 `'full'`）→ 一次 click 的响应可能比 click 本身大两个数量级。我们必须显式决定增量/按需。
- `browser_evaluate`（页面上下文）与 `browser_run_code_unsafe`（服务端进程，自述 RCE-equivalent）**不可合并**。

### 8.2 元素引用方案对比与结论

| 维度 | PW `ref` | CDM `uid` | BU `index` | SH `xpath=` |
|---|---|---|---|---|
| 形态 | `e12`（iframe 内 `f3e12`） | `7_12` | 纯数字 | 位置型绝对 XPath `/html[1]/body[1]/…` |
| 谁有引用 | **仅可见且 receivesPointerEvents** | a11y 树保留节点（`interestingOnly=!verbose`） | 可见且"可交互"（部分元素无条件索引） | 被 LLM 选中且能查到者 |
| 生成 | `(refPrefix??'')+'e'+(++lastRef)` | `` `${loaderId}_${backendNodeId}` `` → 首次分配 `snapshotId_counter` | **直接用 CDP `backend_node_id`** | 逐级 `tag[siblingIndex]` |
| 稳定性机制 | 元素 expando `_ariaRef` 缓存，**role+name 不变即复用** | 跨快照复用 map（同 loaderId+backendNodeId → 同 uid） | backendNodeId 本身 + 选择器索引缓存 | **无**，每次重推理 |
| 失效文案 | `Ref … not found in the current page snapshot. Try capturing new snapshot.` | `Element uid "…" not found on page …` | 不抛异常：`Element index N not available - page may have changed.` | v4 selfHeal **默认关闭**，直接 `{success:false}` |
| 跨 step 稳定性 | 中高 | 中高 | 中（**CDP 会复用 backendNodeId**，源码自述风险） | **低**（官方承认会 breaks） |
| iframe / shadow | `f<frameSeq>` 前缀 + `enter-frame` 递归下钻 | `includeIframes:true`，共用编号空间 | `max_iframes=100` / depth 5，超限告警 | XPath 用 `//` 表 shadow；v3 是占位符，**v4 静默丢弃** |

**结论**：用 aria 语义 ref，**不要**位置型 XPath（SH 是唯一负面样本）；**也不要**裸 `backendNodeId`（BU 的做法）；抄 PW 的「role+name 不变则复用」+ 全局单调递增，或 CDM 的 `loaderId+backendNodeId` 键；ref 只在最近一次快照内有效，失效必须显式报错；只给可交互元素发 ref；必须提供 `find`。

### 8.3 快照与响应体体积：四家都没有可靠硬上限

- PW：零上限，只靠 distill 插件链 + `depth` + 落盘 + `snapshot.mode='none'`
- CDM：`verbose=true` 会关掉 `interestingOnly`，体积跳量级
- BU：唯一确定性硬上限 `max_clickable_elements_length=40000` 字符（另 extract 100000 / evaluate 20000 / 消息层 60000）
- SH：主路径零上限（仅 v3 agent `MAX_TOKENS=70000`）

### 8.4 超时默认值（互相矛盾，必须自己定）

PW：action 5000ms / navigation 60000ms / settle 500ms / `wait_for.time` 上限 30s。CDM：页面操作 5000ms、导航 10000ms、整工具调用 `TOOL_CALL_TIMEOUT_MS=60000`。BU：action 兜底 180s、extract LLM 120s。DSH：MCP client `toolCallTimeoutMs` 默认 **60000**；Stagehand `operationTimeoutMs` 默认 **30000**（action RPC 上再 +10s）。

### 8.5 图片与截图通路

CDM **仅当编码 <2,000,000 字节才内联**，否则落临时文件返回路径（另有 max-width/height 降采样）。PW **零上限、永远 base64 内联**。BU 默认**没有** screenshot 工具（`use_vision=True` 时会 exclude 它），数字高亮默认关闭且截图前移除。
DSH：图片需 attachment store + 模型路由声明 `inputModalities` 含 image，否则整批图片被替换为 `[image unavailable: …]`；内联图片字节不进 model-visible history。

### 8.6 会话归属（DSH 已有硬规范，直接沿用）

只允许一个 provider 注册（第二个同名也失败）；浏览器绑定**确切 live Agent+Session**；attach 独占，被占用时该 activation 永久跳过且后续 turn 不重试；执行期必须拒绝跨 Session（`browser tool belongs to another Session`）；工具名 `mcp__<serverName>__<rawName>`，serverName 匹配 `[A-Za-z0-9_-]{1,32}`；**不要把浏览器工具名显式列进 `toolOrder`**；服务器指令上限 32768 字节。

### 8.7 明确可裁剪（已列入 PLAN 非目标）

性能（trace ×3 + lighthouse）、内存（15 个 heap 工具）、存储（PW 17 个 cookie/storage 工具）、录制/视频/tracing/screencast、坐标视觉点击（PW `--caps=vision`、CDM `experimentalVision` 默认 false）、PDF、测试断言、PWA/扩展/WebMCP、BU 的文件读写工具。

### 8.8 明确「未查到」（不编造）

PW 的快照字符/token 上限与截图字节上限（零命中）；CDM 的页面快照大小上限（零命中，只有 heap snapshot 分页提示）；SH 的 `backwardsCompat`、`ACT_MAX_*`、`domSettled`、独立 VLM 角色、v4 `getAccessibilityTree` 导出（v1–v4 全量产物零命中）；BU 的 `ref` 概念、`TrimmedDom`/`trim_dom`、0.1.x 旧 action 名（0.13.x 已无同名函数）。
