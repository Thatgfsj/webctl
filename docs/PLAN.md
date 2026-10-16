# webctl 项目计划

| 项 | 值 |
|---|---|
| 版本 | 草案 v0.3（2026-10-16） |
| 前身 | `Thatgfsj-easyRep/playwright-crawler-helper`（Python + Playwright，v4.0） |
| 形态 | Rust workspace，单个静态二进制；CLI + MCP server |
| 依据 | 全部硬数据与出处见 [SURVEY.md](SURVEY.md) |
| 规范核实 | **已完成**：「每 1000 行 PR 需审批」在现有仓库中**无成文依据**（SURVEY §1.2）。本计划把它作为**待写入的新项目规则**执行 |
| 工具面 | **已定稿**：核心 20 工具 + ref 方案 + 快照预算 + 超时/图片默认值（见 §3.8；证据 SURVEY §8） |

---

## 1. 目标与非目标

**目标**

1. 重写为 Rust：无 Python、无 Playwright、无 Node、无 WebDriver。
2. **自研 CDP 客户端**，直接驱动 Edge/Chrome/Chromium（本机实测：无 Playwright 可完全驱动，SURVEY §2.2）。
3. **无浏览器环境依然可用**：内置 HTTP/解析 + 协议级会话引擎 + 白名单文档桥完成检索与协议级操作。
4. 面向 AI agent：统一结果信封、稳定元素引用、明确降级语义；同时可作 CLI 与 MCP server。

**非目标（本期不做）**

- 不承诺绕过 Cloudflare/验证码，只提供开关与 hook。
- 不做分布式爬取调度、视频流、移动端手势库。
- 不做 CDP 全量协议覆盖；不实现 `Debugger`/`Profiler`/`HeapProfiler`/`Overlay`/`CSS`/`WebAudio`。
- 不做旧 Python API 兼容层（改为迁移映射表 + 旧仓归档）。

---

## 2. 四层能力阶梯（架构支点）

一套命令面，四种后端；**同一命令跨层语义一致**，agent 工作流不因环境变化而改写，只变 `meta.tier`。

| 层 | 名称 | 依赖 | 能力 | 可用性 |
|---|---|---|---|---|
| **T0** | 内置解析 | 纯 Rust：wreq/reqwest + scraper/html5ever + dom_smoothie | 拉取、正文、链接、表格、表单、JSON/XML/PDF 文本层 | 永远可用 |
| **T1** | 协议级会话引擎 | 纯 Rust | cookie jar、重定向链、自动填表提交、CSRF 提取、multipart、分页、登录态导入导出 | 永远可用 |
| **T2** | 文档桥 | 系统 CLI（白名单） | `pdftotext` / `tesseract` / `pandoc` / `jq` | 有则启用 |
| **T3** | 真浏览器 | 自研 CDP（Edge/Chrome/Chromium） | 导航、a11y 快照、点击、输入、等待、JS 求值、截图/PDF、网络抓包、多标签、上传 | 有 Chromium 时 |

**T1 边界必须诚实**：只覆盖服务端渲染与表单流；SPA 在无浏览器时**必须显式降级**。实测：同一站点静态版可解析 10 条，JS 版静态抓取 0 条（SURVEY §2.4）。

---

## 3. 关键设计决策

### 3.1 协议类型：手写最小子集，不生成全量 ← 最重要的决策

全量 CDP 协议类型 **111,037 行**（`chromiumoxide_cdp`），单个 domain 生成代码即超 1000 行（network 11,795 / page 9,649）。纳入 PR 则任何 PR 都会越线。

**决策**：只为真正调用的约 40 条 command 手写 serde 类型，未知字段忽略，预计 **800–1,200 行（估算）**，独立 crate 交付。

**备选（需显式审批）**：若将来需要全量协议，走"离线生成 + 提交产物"独立 crate，并按 §5.1 的行数口径**单列不计入**。不采用 build.rs 生成（产物不可评审、协议版本钉死）。

### 3.2 依赖选型

| 用途 | 选择 | 理由 |
|---|---|---|
| HTTP | `wreq`（主）/ `reqwest` | wreq 活跃（2026-10-15）、Apache-2.0、浏览器 TLS 指纹；**替代 shell 调 curl**，缩小安全面 |
| HTML 解析 | `scraper` + `html5ever` | ISC，成熟 |
| 正文提取 | `dom_smoothie` | MIT，活跃 |
| HTML→文本 | `rust-html2text` | MIT，活跃 |
| CDP 传输 | `tokio-tungstenite` | prior art 实测传输层仅 160 行，自研不划算 |
| MCP | `rmcp` 3.5.1 | 官方 SDK；**`transport-io` 必须显式开启** |
| CLI/序列化 | `clap` / `serde` / `schemars` | schemars 直接生成 MCP tool schema |

**不采用**：WebDriver 系（`fantoccini`/`thirtyfour`，需额外 driver 进程）；依赖 `chromiumoxide`（协议 crate 半年未更新，与自研目标冲突）；任何 Python/Node 运行时。

### 3.3 浏览器 profile 策略

**不使用默认 profile attach**：Chrome/Edge 136 起默认 profile 禁止开启 remote-debugging（SURVEY §1.3）。只支持受管 profile；登录态复用由 `profile import`（复制 cookie）实现。

### 3.4 结果信封与降级语义

```json
{ "ok": true, "data": {}, "error": "", "code": "OK",
  "meta": { "tier": "T3", "trust": "untrusted_page_content",
            "degraded_from": "T3", "stale": false, "elapsed_ms": 412 } }
```

- 错误码：沿用 `OK / TIMEOUT / NETWORK / BLOCKED / BAD_URL / PARSE / BROWSER_CRASH`，新增 `NO_BROWSER / DEGRADED / TOOL_MISSING / STALE_REF / SESSION_LOST`。
- **禁止静默降级**：任何降级必须体现在 `meta.degraded_from` + `code`。
- 页面内容一律标 `trust: untrusted_page_content`。

### 3.5 安全红线（三条）

1. **不执行任意 shell 字符串**：T2 仅内部枚举命令 + 参数数组，路径 `canonicalize` 后校验；`--no-external` 可整体关闭。
2. **`eval` JS 默认关闭**：需显式 `--allow-eval`；返回值强制 JSON 化，不落原始堆栈。
3. **凭据显式开关**：`cookies export` 需 `--yes`，文件权限 600；信封与日志中 cookie 值一律脱敏。

### 3.6 DSH / MCP 集成硬约束

- 每次工具调用**默认 60s 超时** → 工具必须远快于此，长任务分片或显式配置。
- stdio **先起探测进程、再起服务进程** → 服务必须能被启动两次而不冲突（无独占锁/端口占用）。
- 服务器指令 **≤ 32KB**；工具名 `mcp__<serverName>__<rawName>`，需满足 function-name 契约。

### 3.7 进程与状态模型

浏览器按会话懒启动（冷启动到 CDP 就绪 **292–313ms** 实测，可接受）；会话状态落盘（`state dir + 文件锁 + TTL`）；CLI 与 MCP 共享状态；`--session <name>` 跨进程复用。

### 3.8 工具面设计决策（定稿，证据 SURVEY §8）

**核心工具集约 20 个**：navigate / back、tabs、snapshot、**find**、click、hover、fill、**fill_form**、select_option、press_key、drag、upload_file、handle_dialog、scroll、wait_for、evaluate、screenshot、console_messages、network_requests、extract（含 search_page）。其余（性能 trace、内存 heap、存储、录制/视频、视觉坐标点击、PDF、测试断言、PWA/扩展）列入非目标。

| 决策 | 取值 | 依据 |
|---|---|---|
| 元素引用 | aria/a11y 语义 ref；**role + accessible name 不变则复用同一 ref**；全局单调递增编号；**仅给可交互元素发 ref** | PW 方案最稳；Stagehand 的位置型 XPath 是唯一负面样本；browser-use 的 `index` 就是裸 `backendNodeId`，而 CDP 会复用 backendNodeId（其源码自述风险） |
| ref 失效 | 显式 `STALE_REF` + "重新获取快照"指引，**禁止静默失败** | 四家一致做法 |
| 快照减量 | 字符预算 + 截断标记 + `depth` 限制 + 落盘选项 + `find` 窗口内搜索 | 四家**都没有可靠硬上限**（仅 browser-use 有 40000 字符截断），我们必须更严 |
| 动作后是否附快照 | 默认**只给增量摘要**，整表按需获取 | PW 默认 `full`，一次 click 的响应可能比 click 本身大两个数量级 |
| 超时默认 | 页面操作 5s、导航 10s、`wait` 上限 30s、单次 MCP 调用预算 < 60s | 四家默认值互相矛盾（PW 5s/60s、CDM 5s/10s/60s、BU 180s）；DSH MCP `toolCallTimeoutMs` 默认 60s |
| 图片 | 超过阈值**落盘返回路径**，不无脑内联；无图片能力时给可读诊断 | CDM 的 2MB 内联阈值；DSH 需 attachment store + 模型声明 `inputModalities` 含 image |
| `eval` | 只做**页面上下文**求值，且默认关闭 | PW 的 `run_code_unsafe` 是服务端进程、自述 RCE-equivalent，**不可合并** |
| 会话归属 | 单 provider；绑定确切 live Agent+Session；attach 独占；跨 Session 调用拒绝；工具名 `mcp__webctl__*`；**不写入 `toolOrder`** | DSH 既有硬规范 |

---

## 4. 模块划分

### 4.1 拆分判据（先定纪律，再定目录）

依据 DSH 的 capability-seam 纪律——**"角色不独立演进就不要预先拆包"**——本项目采用：

> **只有当两个模块 (a) 变更来源或节奏明显不同，且 (b) 依赖方向单向无环时，才拆成独立 crate；否则同一 crate 内用目录分隔即可。**

"每个 PR ≤1000 行"是**评审粒度**约束，靠目录与 PR 边界满足，**不靠增加 crate 数量**。

### 4.2 布局（7 个 crate，比初稿减少 4 个）

```
webctl/
├─ crates/
│  ├─ webctl/           # bin：CLI 入口 + MCP 入口（feature 切换）
│  ├─ webctl-core/      # envelope/错误码/配置/能力探测/tier 路由/会话与状态
│  ├─ webctl-web/       # T0+T1：HTTP、编码、cookie、解析、正文、链接/表格/表单、协议会话引擎、T2 白名单桥
│  ├─ webctl-cdp/       # T3 底座：线协议信封、传输、分发、事件、target 会话、浏览器发现与启动
│  ├─ webctl-cdp-types/ # T3：手写最小协议类型（唯一因"来源不同 + 体量"而拆的 crate）
│  ├─ webctl-browser/   # T3 语义层：快照、交互原语、截图/PDF、网络捕获
│  └─ webctl-mcp/       # rmcp server + 工具 schema + SKILL.md 生成
└─ tests/               # mock CDP server、fixture 站点、golden、E2E 基线
```

**拆包理由（逐条）**

| crate | 为什么独立 |
|---|---|
| `webctl-cdp-types` | 变更来源是 CDP 协议版本，与业务逻辑无关；体量可能与其余全部相当 |
| `webctl-cdp` | 依赖方向单向（只依赖 types），且是唯一有"协议语义状态机"复杂度的层 |
| `webctl-browser` | 面向 AI 的语义（ref/快照/交互），变更节奏跟随模型用法而非协议 |
| `webctl-web` | T0/T1 完全不依赖 T3，且必须能在无浏览器环境独立测试 |
| `webctl-mcp` | 工具面契约（DSH function-name、schema）独立演进 |
| `webctl-core` | 被所有层依赖，必须无上层依赖 |
| `webctl`（bin） | 仅组装与参数解析 |

**禁止**：协议类型、传输、会话语义、工具面跨层混改（同一 PR 只动一类）。

---

## 5. PR 拆分与交付顺序

### 5.1 行数口径（先写死，可机械校验）

- 口径 = `git diff --shortstat` 的 `additions + deletions`。
- **不计入**：`webctl-cdp-types` 下生成/批量录入的协议类型文件、`docs/` 纯文档、测试 fixture 数据文件。**但必须在 PR 描述里写明实际行数**，避免成为后门。
- **目标 ≤1000 行**；越线需维护者在 issue 或 PR 中显式批准并说明理由。

### 5.2 PR 清单

| PR | 内容 | 预估行数 | 依赖 | 验收标准 |
|---|---|---|---|---|
| 01 | workspace 骨架、envelope/错误码、`caps` 能力探测、CI | 450 | — | `webctl caps --json` 在无浏览器机输出正确分层；三平台 `cargo build` 绿 |
| 02 | T0 传输：wreq 封装、重定向、cookie jar、编码、内容类型路由 | 700 | 01 | golden：多编码/压缩/重定向用例全绿 |
| 03 | T0 解析：正文提取、链接/表格/表单、markdown 输出 | 800 | 02 | 离线 fixture 站 golden 全绿 |
| 04 | T1 协议会话引擎：cookie 持久化、表单提交、CSRF 提取 | 700 | 02 | 本地 axum 表单站「登录→翻页→提交」端到端断言 |
| 05 | CDP 线协议信封（`MethodCall`/`CallId`/`Command`/`Event`/`Response`）+ 错误类型 | 550 | 01 | 帧编解码单测；与真实 Edge 握手成功 |
| 06 | CDP 传输：WebSocket 读写、写队列、超时、pump 任务 | 400 | 05 | 对真实 Edge 收发 `Browser.getVersion` |
| 07 | CDP 分发：id 分配、pending 表、响应/错误解复用、sessionId 路由 | 700 | 06 | 并发 100 命令不串号；超时正确归类 |
| 08 | CDP 事件：监听注册、类型过滤、Stream 广播、未知事件容忍 | 450 | 07 | 事件顺序与去重断言 |
| 09 | target-session：Target 发现、`attachToTarget(flatten)`、多会话生命周期 | 800 | 08 | 多标签并发会话隔离断言 |
| 10 | 浏览器发现/启动：可执行文件探测、受管 profile、spawn、端点探测、清理与崩溃恢复 | 900 | 09 | 冷启动→就绪→关闭；僵尸进程回收 |
| 11a | 最小协议类型（一）：Page / Runtime / Target | 900 | 05 | 序列化断言（不计入行数口径，另附实际行数） |
| 11b | 最小协议类型（二）：DOM / Input / Accessibility / Network / Emulation | 900 | 05 | 同上 |
| 12 | page-nav：navigate、生命周期等待、frame | 600 | 10, 11a | 导航到 fixture 站并拿到 title |
| 13 | page-capture：截图、PDF、Emulation（移动仿真四件套） | 500 | 11b, 12 | 截图非空；**超阈值落盘返回路径**（不无脑内联）；移动视口 `innerWidth` 正确（注意 980 陷阱） |
| 14 | snapshot + `find`：a11y 语义 ref（role+name 复用、仅可交互元素）、字符预算与截断标记、快照内搜索 | 900 | 13 | 大页面快照有界（预算生效且带截断标记）；ref 失效返回 `STALE_REF` + 重取指引；`find` 返回祖先路径与上下文 |
| 15 | 交互原语：click/hover/fill/**fill_form**/select/press/drag/upload/scroll/wait/handle_dialog（几何 + Input） | 900 | 14 | 批量填表一次调用完成；真实站点点击与中文输入生效；dialog 正确处置 |
| 16 | Network 捕获：事件 + `getResponseBody` + 缓冲上限；console 消息分级 | 800 | 11b, 09 | 抓到 XHR/Fetch 与响应体；console 分级；超限截断 |
| 17 | MCP server（rmcp）+ 工具 schema + SKILL.md 生成 + 会话归属与命名契约 | 500 | 15, 16 | DSH 挂载后 `mcp__webctl__*` 出现并可调用；服务器指令 ≤32KB；不写入 `toolOrder` |
| 18 | T2 文档桥：白名单、`canonicalize`、`--no-external` | 600 | 02 | 白名单外命令被拒；关闭开关后不外部调用 |
| 19 | 安全红线：eval 开关、cookie 导出脱敏、trust 标记、日志脱敏 | 500 | 15 | 越权用例全部被拒 |
| 20 | 测试基建：mock CDP server、fixture 站、golden、探针固化 | 800 | 01 | 无浏览器 CI 下 L2 逻辑可测 |
| 21 | 打包发布：cargo-dist、musl 静态、三平台、双语 README | 500 | 全 | Release 资产可下载即用；启动基线防回归 |

**验收锚点（必须过）**：`quotes.toscrape.com/js/` 静态抓取 0 条 → webctl 必须拿到 **10 条**（实测基线见 SURVEY §2.4）。

---

## 6. 工作流规范

### 6.1 现存的真实条款（必须遵守）

| 条款 | 出处 |
|---|---|
| PR 必须关联 issue，且先在 issue 领取，否则不通过 | dsh-ops `docs/CONTRIBUTING.md` |
| feat 前先写 issue，维护团队批准后再动手 | 同上 |
| 联动层单开逻辑区域，与核心完全解耦 | 同上 |
| 拆分独立变更；修复落在引入它的那个 PR | DSH `AGENTS.md` |
| ≥2 层依赖链必须用官方 stack，部分落地需显式边界 PR | DSH `.agents/notes/.../native-github-stacks...md` |
| **一次 PR 只加一类**变更，禁止"顺手规范化" | `dsh-ecosystem-spec/CONTRIBUTING.md` §4 |
| 治理/挂载 revision 升级必须是独立 PR | `dsh-ecosystem-spec/governance/rules.md` §3 |
| 角色不独立演进就不预先拆包 | DSH `AGENTS.md`（capability seam） |
| 业务规则独立成包；新行为走插件而非改主干 | DSH `packages/AGENTS.md` |

### 6.2 待写入的新规则（本计划的执行依据）

「每 1000 行 PR 需审批」**目前没有仓库依据**（SURVEY §1.2）。为使其可执行，建议按下列措辞写进项目的 `CONTRIBUTING.md`：

> 单个 PR 的 `git diff --shortstat` 改动行数（additions + deletions）目标不超过 1000 行；超过时需维护者在关联 issue 中显式批准并写明理由。生成产物、协议类型批量录入、纯文档与测试 fixture 不计入该口径，但必须在 PR 描述中列明实际行数。

注意两点风险（来自调查）：
- 组织级审批门禁是**加权 2 分制**（`T-Auto/.github` 的 `approval-policy.json`），与行数无关；新规则若要与门禁联动，须改 CI 而非只写文档。
- 策略类改动**合并进默认分支后才生效**，不能在自身 PR 内改变门禁。

---

## 7. 验收门禁

| 阶段 | 门禁命令 | 通过标准 |
|---|---|---|
| 每 PR | `cargo fmt --check && cargo clippy -- -D warnings && cargo test` | 全绿 |
| 无浏览器 lane | `WEBCTL_DISABLE_BROWSER=1 cargo test --features no-browser` | T0/T1/T2 用例全部通过 |
| L2 离线 lane | `cargo test --features mock-cdp` | mock CDP 回放用例全绿 |
| 真机 E2E | `./tests/e2e/run-edge.sh` | Edge 全链路通过（探针固化自 SURVEY §2.2） |
| 冷启动基线 | `./tests/bench/startup.sh` | 不劣于 P0 记录基线（292–313ms） |
| 体积基线 | `cargo build --release --target x86_64-unknown-linux-musl` | 基线在 P0 确定后防回归 |
| PR 行数 | `git diff --shortstat` 对照 5.1 口径 | ≤1000 或已获批准 |

---

## 8. 风险与缓解

| 风险 | 缓解 |
|---|---|
| Edge/Chrome 升级导致 CDP 漂移 | 只用稳定子集、忽略未知字段；`caps --protocol` 打印版本与实测能力 |
| 协议类型体量失控 | 手写最小子集；全量走独立生成 crate 并按 5.1 单列 |
| a11y ref 失效 | ref 优先 + CSS/XPath 兜底；失效返回 `STALE_REF` 并提示重新快照 |
| headless 被识别 | 有头模式 + 受管真实 profile + CDP stealth；不承诺破解 |
| 会话/进程泄漏 | 文件锁 + TTL + 关停路径测试 + 僵尸回收 + 30 分钟长跑用例 |
| 移动仿真陷阱 | 需设备指标 + viewport meta + touch + UA 多件套，否则 `innerWidth` 恒为 980 |
| MCP 60s 超时 | 工具调用分片；长任务显式配置 |
| 规则无成文依据导致复现争议 | 先按 6.2 写入 CONTRIBUTING，再开工 |

---

## 9. 待验证清单（不得当作已证实）

1. `wreq` 在 Rust 1.98 + rustls 下的编译与特性组合。
2. 大响应（>10MB DOM / 网络 body）下 CDP WebSocket 的内存表现。
3. Edge headless 下 `Fetch` 域拦截改写的稳定性。
4. DSH MCP **探测进程**与 session 文件锁的兼容性。
5. 30 分钟以上会话的浏览器是否泄漏。
6. `Lightpanda` 作为可选后端：CDP 覆盖度与 AGPL-3.0 许可结论。

---

## 10. 开放决策（未回复则按默认值执行）

| # | 决策 | 默认值 |
|---|---|---|
| 1 | 项目名与仓库 | `webctl`，新建 `Thatgfsj/webctl`；旧仓仅加迁移说明 |
| 2 | T2 文档桥 | 保留，白名单 + 可关闭 |
| 3 | 旧 Python 版 | 归档不兼容，只出迁移映射表 |
| 4 | 1000 行规则 | 按 §6.2 写入 CONTRIBUTING 后执行 |
