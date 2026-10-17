# webctl 项目计划

| 项 | 值 |
|---|---|
| 版本 | **草案 v0.4（2026-10-16）** |
| 前身 | `Thatgfsj-easyRep/playwright-crawler-helper`（Python + Playwright，v4.0） |
| 形态 | Rust workspace；发布 **2 个二进制**（`webctl` 主 CLI 含 `mcp` 子命令、`webctl-mcp` 独立 MCP 入口）；三平台（Linux 为 glibc 动态链接——musl 静态暂缓，见 [#90](https://github.com/Thatgfsj/webctl/issues/90)）；**零外部进程、零浏览器依赖** |
| 依据 | 历史硬数据见 [SURVEY.md](SURVEY.md)；**但 §0 的方向变更已作废其中的 T2/T3 相关结论** |
| Spike | [wreq 验证报告](spikes/wreq.md)（HTTP 选型已定点） |
| 决策记录 | [docs/adr/](adr/README.md)：ADR-0001 单层内置 · ADR-0002 不做 JS 引擎 · ADR-0003 选择器取代 ref · ADR-0004 礼貌与限速 |
| 工具面 | **已定稿**：15 个内置工具（§3.7），无截图、无坐标交互、无 JS 执行 |
| 规范核实 | 「每 1000 行 PR 需审批」在现有仓库中**无成文依据**（SURVEY §1.2）；本计划把它作为**待写入的新项目规则**执行，措辞已落地在 [CONTRIBUTING.md](../CONTRIBUTING.md) §2 |

---

## 0. 方向变更（v0.3 → v0.4）—— 必读

**v0.3 的四层阶梯（T0 内置 / T1 会话 / T2 外部 CLI 桥 / T3 真浏览器）已废弃。v0.4 只保留"全部内置"的单一层：所有能力在进程内实现，不启动任何外部进程。**

取消的内容：

| 被取消 | v0.3 中的位置 |
|---|---|
| 自研 CDP 客户端、Edge/Chrome 发现与启动、受管 profile | T3，PR 05–10 |
| 手写最小 CDP 协议类型 | T3，PR 11a/11b |
| 导航、快照与 ref、交互原语、截图/PDF 打印、移动仿真、网络抓包、console 抓取 | T3，PR 12–16 |
| 外部 CLI 白名单桥（`pdftotext` / `tesseract` / `pandoc` / `jq`）、`--no-external` 开关 | T2，PR 18 |
| `--allow-eval`（进程内没有 JS 引擎，无 JS 可执行） | 安全红线之一 |

**理由**：能内置的都内置之后，外部进程与外部浏览器带来的复杂度不再划算——协议漂移、进程/会话泄漏、冷启动 292–313 ms、跨平台浏览器探测、封禁与反检测博弈、白名单与 `canonicalize` 的安全面。进程内实现更快、更可测、分发更简单。

**代价（必须诚实记录，不许悄悄消失）**

1. **JS 渲染页面拿不到。** 无 JS 引擎 ⇒ `quotes.toscrape.com/js/` 仍为 **0 条**（SURVEY §2.4）。v0.3 的验收锚点「必须拿到 10 条」**作废**。异步 XHR 注入的内容、SPA 首屏同样拿不到。
2. **没有渲染器就没有像素与布局。** `screenshot`、`print to PDF`、按坐标点击、真实几何元素匹配从工具面移除；`fill_form` 退化为按表单字段名填值并提交，而不是"点按钮"。
3. **OCR 不再支持。** 纯 Rust 无成熟 OCR（SURVEY §2.1 显示本机 `tesseract` 缺失）。PDF 只做文本层，扫描件明确 `UNSUPPORTED`。
4. **SURVEY 的 CDP 证据退化为历史记录。** §2.2 / §2.3 / §3 / §8 不再指导实现，保留以备将来复查。
5. **反爬姿态变弱。** 没有真实浏览器指纹，只有 TLS 指纹（wreq）与请求头策略；不承诺绕过 Cloudflare/验证码，遇到即 `BLOCKED`。

**可逆性（保留缝隙，本期不做）**：将来若要 JS 渲染，走**可选内置引擎**（Boa 纯 Rust / rquickjs + 自建最小 DOM/BOM），仍不引入外部进程与外部浏览器。按 capability-seam 纪律，**现在不为它预先拆包**。

---

## 1. 目标与非目标

**目标**

1. 纯 Rust：无 Python、无 Node、无浏览器、无外部 CLI 进程。
2. 内置取回 + 解析 + 协议级会话引擎，覆盖服务端渲染站点与表单流。
3. 面向 AI agent：统一结果信封、结构化抽取、明确降级语义（JS 页面必须显式降级，不得伪造成功）。
4. 同一套命令面同时提供 CLI 与 MCP server。

**非目标（本期不做）**

- JS 执行与渲染、DOM 事件模拟、SPA 水合。
- 截图、页面 PDF 打印、真实布局几何、移动仿真。
- OCR、视频流、分布式调度、移动端手势、PWA/扩展/WebMCP。
- 绕过 Cloudflare/验证码（只提供请求头/限速/重试等合规手段与明确失败）。
- 旧 Python API 兼容层（改为迁移映射表 + 旧仓归档）。

---

## 2. 能力模型：单层内置

v0.4 没有"层"了，只有一组**内置能力域**，全部在进程内：

| 能力域 | 内容 | 关键边界 |
|---|---|---|
| **取回** | HTTP(S)、重定向链、cookie jar、编码/压缩、内容类型路由、并发与限速 | 只走网络，无渲染 |
| **解析** | HTML→DOM、正文、链接、表格、表单、markdown、JSON/XML | 只解析已取回的字节 |
| **会话** | cookie 持久化、自动填表提交、CSRF、multipart、分页、登录态导入导出 | 只覆盖服务端渲染与表单流 |
| **文档/数据** | PDF 文本层、JSON 查询子集、HTML→文本/ markdown | 扫描件与复杂二进制 → `UNSUPPORTED` |
| **接口** | CLI、MCP（rmcp）、SKILL.md | 契约见 §3.6 |

`meta.engine` 恒为 `"builtin"`（为将来的可选内置引擎预留取值，如 `"builtin+js"`）。

---

## 3. 关键设计决策

### 3.1 无外部进程 ⇒ 安全面收敛

- 不执行任何子进程：v0.3 的白名单/canonicalize/注入防护**整类风险消失**（约束仍在：代码里不得出现 `Command` 调用）。
- 无 JS 引擎 ⇒ 无 `eval` 通路，**RCE-equivalent 风险为零**。
- 仍然保留的红线：凭据显式开关 + 脱敏、页面内容 `trust: untrusted_page_content`、日志脱敏。

### 3.2 依赖选型

| 用途 | 选择 | 状态 |
|---|---|---|
| HTTP | `wreq` 0.16.1（主）/ `reqwest` | **已验证**（[spike 报告](spikes/wreq.md)）：Apache-2.0、MSRV 1.98、默认特性即可；构建期需 cmake + C/C++ + libclang |
| TLS 指纹档 | `wreq-util` 0.2.0 | 提供 `Emulation`（Chrome149 / Firefox149 / Safari26 / OkHttp5…）；JA3/JA4 实测互不相同且不同于 reqwest |
| HTML 解析 | `scraper` + `html5ever` | 成熟 |
| 正文提取 | `dom_smoothie` | 成熟 |
| HTML→文本 | `rust-html2text` | 成熟 |
| PDF 文本层 | `pdf-extract` 或 `lopdf` | **待 spike 验证**（纯 Rust，质量未知） |
| JSON 查询 | `serde_json` + 自研小子集（或 `jmespath`） | 待评估 |
| MCP | `rmcp` 3.5.1 | 官方 SDK；**`transport-io` 必须显式开启** |
| CLI/序列化 | `clap` / `serde` / `schemars` | `schemars` 直接生成 MCP tool schema |

**不采用**：任何浏览器、WebDriver、CDP 客户端、外部 CLI 进程、Python/Node 运行时。

### 3.3 会话与状态

`--session <name>`：状态落盘（state dir + 文件锁 + TTL + 权限 700），CLI 与 MCP 共享同一状态，跨进程复用。会话文件损坏/过期必须给出可读错误，不得静默新建。

### 3.4 结果信封与降级语义

```json
{ "ok": true, "data": {}, "error": "", "code": "OK",
  "meta": { "engine": "builtin", "trust": "untrusted_page_content",
            "degraded_from": null, "stale": false, "elapsed_ms": 412 } }
```

- 错误码（封闭集）：`OK / TIMEOUT / NETWORK / BLOCKED / BAD_URL / PARSE / DEGRADED / UNSUPPORTED / SESSION_LOST / LIMIT / INTERNAL`。
  - 相对 v0.3：删除 `BROWSER_CRASH / NO_BROWSER / TOOL_MISSING / STALE_REF`，新增 `UNSUPPORTED`（能力不存在，如 JS 渲染、OCR）与 `LIMIT`（命中缓冲/尺寸上限）。
- **禁止静默降级**：任何能力缺失或降级必须体现在 `code` + `meta.degraded_from`。
- **失败可区分**：空结果 ≠ 解析失败 ≠ 能力不支持 ≠ 被拦截，四者码不同。
- 页面内容一律 `trust: untrusted_page_content`。

### 3.5 选择器与"引用"：没有 live DOM 的替代方案

v0.3 的 aria ref 依赖"最近一次快照"的 live 页面，**在无浏览器模型下不成立**，改为：

| 场景 | 方案 |
|---|---|
| 抽取 | CSS 选择器 + 文本/正则（`extract` / `search`） |
| 表单交互 | `(form_index, field_name)` 稳定寻址；`fill_form` 一次完成多字段 |
| 结果可追溯 | 每次返回元素的结构化路径（tag/索引/祖先链）与文本上下文 |
| 失效 | 不存在跨请求的"引用失效"：每次解析都是新的；`stale` 恒 false |

即：**用选择器取代 ref**，并明确"同一选择器在不同时刻可能命中不同元素"这一固有语义。

### 3.6 DSH / MCP 集成硬约束（沿用 v0.3）

- 单次工具调用默认 **60s** 超时 ⇒ 工具必须远快于此；长任务分片或显式配置。
- stdio **先起探测进程、再起服务进程** ⇒ 服务必须能被启动两次而不冲突（无独占锁/端口占用）。
- 服务器指令 **≤ 32KB**；工具名 `mcp__webctl__<rawName>`，满足 function-name 契约。
- 会话归属：单 provider、绑定确切 live Agent+Session、跨 Session 调用拒绝；**不写入 `toolOrder`**。

### 3.7 工具面（15 个，定稿）

| # | 工具 | 说明 |
|---|---|---|
| 1 | `fetch` | 取回 URL，返回状态、头、重定向链、内容或落盘路径 |
| 2 | `extract` | 选择器/正文提取，返回结构化结果 + markdown/文本 |
| 3 | `search` | 页面内文本/正则搜索，返回命中 + 结构化路径 + 上下文 |
| 4 | `links` | 链接清单（绝对化、rel、可选过滤） |
| 5 | `tables` | 表格结构化（行列、跨行跨列语义确定） |
| 6 | `forms` | 表单与字段清单（类型、默认值、action/method） |
| 7 | `fill_form` | 一次调用填表并提交（含隐藏字段与 CSRF） |
| 8 | `paginate` | 分页遍历（next 识别 + 上限） |
| 9 | `session` | 会话查看/保存/加载/清理（cookie 持久化） |
| 10 | `cookies` | 查看/导出 cookie（导出需 `--yes`、权限 600、值脱敏） |
| 11 | `json_query` | 内置 JSON 查询子集（替代 `jq`） |
| 12 | `doc_text` | PDF / HTML / XML 文本层提取（替代 `pdftotext`/`pandoc`） |
| 13 | `download` | 文件下载到指定目录（尺寸上限、类型白名单、落盘路径） |
| 14 | `robots` | robots.txt 与站点策略查询（礼貌爬取依据） |
| 15 | `caps` | 内置能力与版本自检 |

**明确不做**：screenshot / pdf 打印 / click / hover / drag / press_key / upload_file / handle_dialog / wait_for / evaluate / console_messages / network_requests（这些都是浏览器语义）。

### 3.8 限速、礼貌与封禁姿态

- 默认并发上限（每主机 2）、默认最小请求间隔、`Retry-After` 尊重、指数退避上限。
- 默认解析 `robots.txt`（可显式关闭并记录在信封里）。
- 明确不承诺绕过 Cloudflare/验证码：识别到拦截页 → `BLOCKED` + 可读诊断。

---

## 4. 模块划分

### 4.1 拆分判据（沿用 v0.3）

> 只有当两个模块 (a) 变更来源或节奏明显不同，且 (b) 依赖方向单向无环时，才拆成独立 crate；否则同一 crate 内用目录分隔即可。

### 4.2 布局（4 个 crate，v0.3 的 7 个中 3 个随 T3 一起删除）

```
webctl/
├─ crates/
│  ├─ webctl/        # bin：CLI 入口 + MCP 入口（feature 切换）
│  ├─ webctl-core/   # envelope/错误码/配置/能力探测/会话与状态/安全与脱敏
│  ├─ webctl-web/    # 取回 + 解析 + 会话 + 文档/数据（全部内置）
│  └─ webctl-mcp/    # rmcp server + 工具 schema + SKILL.md 生成
└─ tests/            # fixture 站、golden、mock HTTP
```

依赖方向：`bin → mcp → core`、`bin → web → core`，`core` 不依赖任何上层。

**禁止**：接口契约与内核语义跨层混改（同一 PR 只动一类）。

---

## 5. PR 拆分与交付顺序

### 5.1 行数口径（先写死，可机械校验）

- 口径 = `git diff --numstat` 的 `additions + deletions`（实现见 [`scripts/pr-size.sh`](../scripts/pr-size.sh)）。
- **不计入**：`docs/**`、`crates/*/tests/fixtures/**`、`Cargo.lock`、生成的 `.github/workflows/release.yml`、`CHANGELOG.md`。**但必须在 PR 描述里写明实际行数**。
- **目标 ≤1000 行**；越线时 CI 的 `pr-size` 作业失败，维护者在关联 issue 批准后打 `lines: exempt` 标签放行。
- 本规则与组织级「加权 2 分制」门禁**并行**，互不替代（见 [CONTRIBUTING §2](../CONTRIBUTING.md)）。

### 5.2 PR 清单（v0.4 重切，14 个）

| PR | 内容 | 预估 | 依赖 | 验收标准 |
|---|---|---|---|---|
| 01 | workspace 骨架、envelope/错误码、`caps`（内置能力自检）、CI | 450 | — | `webctl caps --json` 报告内置能力；三平台 `cargo build` 绿 |
| 02 | 内置取回：wreq 封装、重定向链、cookie jar、编码、内容类型路由 | 700 | 01 | 多编码/压缩/重定向 golden 全绿 |
| 03 | [#3](https://github.com/Thatgfsj/webctl/issues/3) | [#54](https://github.com/Thatgfsj/webctl/pull/54) | 已合并 | `5afc240` | 解析层；两轮否决（XML 深嵌套栈溢出 → 8MiB 专用栈线程；冲突致 CI 不触发 + `ContentKind::Text` 未覆盖）后通过 |
| 04 | 协议级会话引擎：cookie 持久化、表单提交、CSRF、multipart | 700 | 02 | 本地表单站「登录→翻页→提交」端到端断言 |
| 05 | [#34](https://github.com/Thatgfsj/webctl/issues/34) | [#63](https://github.com/Thatgfsj/webctl/pull/63) | **修复中** | — | state dir/锁/TTL；审阅否决：`sessions` 子目录 symlink 可越界（`purge_expired` 删到 state dir 外）+ state dir 未真正单一入口 |
| 06 | [#35](https://github.com/Thatgfsj/webctl/issues/35) | — | **实现中** | — | 抽取引擎；依赖 PR 03 已清空 |
| 07 | 文档与数据内置：PDF 文本层、`json_query`、格式转换 | 600 | 03 | PDF 文本层用例；扫描件 `UNSUPPORTED` |
| 08 | 礼貌与限速：robots、并发/间隔上限、退避、`BLOCKED` 识别 | 500 | 02 | 限速生效；拦截页归类为 `BLOCKED` |
| 09 | SPA/JS 降级语义：检测与 `DEGRADED` + `UNSUPPORTED` 传播 | 400 | 06 | JS 页必须 `DEGRADED`，不得返回"0 条成功" |
| 10 | CLI 体验与配置：配置档、代理、输出格式、退出码 | 500 | 05 | 配置优先级与退出码用例全绿 |
| 11 | MCP server：15 工具 schema、SKILL.md、DSH 契约、会话归属 | 600 | 09,10 | DSH 挂载后 `mcp__webctl__*` 可调用；≤32KB；不写 `toolOrder` |
| 12 | 安全与凭据：cookie 导出、脱敏、日志、trust 标记 | 500 | 11 | 越权用例全部被拒 |
| 13 | 测试基建：fixture 站、mock HTTP、golden、离线 lane | 700 | 01 | 无网络 CI 下 L0 用例可跑 |
| 14 | 打包发布：cargo-dist、musl 静态、三平台、双语 README | 500 | 全 | Release 资产下载即用；基线防回归 |

### 5.2.1 交付记录（滚动更新）

> 依据：`gh pr list --state merged` 的真实 merge sha（squash 提交）+ 各 PR 的**独立审阅**结论。
> 最后更新：2026-10-17（**PR 03–14 全部落地**）

| PR | 议题 | PR | 状态 | merge sha | 备注 |
|---|---|---|---|---|---|
| 01 | — | [#40](https://github.com/Thatgfsj/webctl/pull/40) | 已合并 | `f669365` | 骨架 + 信封/错误码 + caps + CI |
| 02 | — | [#44](https://github.com/Thatgfsj/webctl/pull/44) | 已合并 | `45aa932` | 取回层：wreq/重定向链/cookie/编码/内容类型 |
| 03 | [#3](https://github.com/Thatgfsj/webctl/issues/3) | [#54](https://github.com/Thatgfsj/webctl/pull/54) | 已合并 | `5afc240` | 解析层；XML 深嵌套改为深度预检 + 独立栈线程（1MiB 栈会 SIGABRT） |
| 04 | [#4](https://github.com/Thatgfsj/webctl/issues/4) | [#50](https://github.com/Thatgfsj/webctl/pull/50) | 已合并 | `7e0480f` | 会话引擎；首轮被否后补 SESS-15/FORM-13…19 |
| 05 | [#34](https://github.com/Thatgfsj/webctl/issues/34) | [#63](https://github.com/Thatgfsj/webctl/pull/63) | 已合并 | `820df36` | state dir 单一入口 / 权限 / flock / TTL / 路径防护；**首轮被否**（`sessions` 子目录 symlink 越界真删了外部文件），复审以逐字节快照 + 反向对照确认修复 |
| 06 | [#35](https://github.com/Thatgfsj/webctl/issues/35) | [#71](https://github.com/Thatgfsj/webctl/pull/71) | 已合并 | `5a1b4dd` | 抽取引擎；**首轮被否**（零命中扫描绕过墙钟预算：1MB 零命中 budget=1ms 返回 `OK`/4032ms），复审做 180 组选择器 + 1144 组正则等价比对 |
| 07 | [#36](https://github.com/Thatgfsj/webctl/issues/36) | [#49](https://github.com/Thatgfsj/webctl/pull/49) | 已合并 | `121d423` | PDF 文本层 + `json_query`；DOC-09 已收口（见 #84） |
| 08 | [#37](https://github.com/Thatgfsj/webctl/issues/37) | [#51](https://github.com/Thatgfsj/webctl/pull/51) | 已合并 | `2a211d4` | 礼貌层；首轮被否（HostGate 排队失效 + robots 两处）后修复 |
| 09 | [#38](https://github.com/Thatgfsj/webctl/issues/38) | [#73](https://github.com/Thatgfsj/webctl/pull/73) | 已合并 | `3c36b54` | 降级语义；以 stacked PR 开工、PR 06 合并后 `--onto` 重放；L1 真站锚点实测通过 |
| 10 | [#39](https://github.com/Thatgfsj/webctl/issues/39) | [#60](https://github.com/Thatgfsj/webctl/pull/60) | 已合并 | `39cf08f` | CLI 与配置；审阅者 38/38 独立探针 |
| 11 | [#19](https://github.com/Thatgfsj/webctl/issues/19) | [#78](https://github.com/Thatgfsj/webctl/pull/78) | 已合并 | `8998044` | MCP server + 15 工具 + `SKILL.md` + caps 诚实性（#68）；复审从 PLAN §3.7 独立解析名单并与 `tools/list` 逐字比对 |
| 12 | [#21](https://github.com/Thatgfsj/webctl/issues/21) | [#79](https://github.com/Thatgfsj/webctl/pull/79) | 已合并 | `918a120` | 安全与凭据；复审自造跨域/越界 symlink 负面样本并独立复现两次植入实验 |
| 13 | [#22](https://github.com/Thatgfsj/webctl/issues/22) | [#55](https://github.com/Thatgfsj/webctl/pull/55) | 已合并 | `e899c2f` | 测试基建；合并后暴露扫描器误报 → #64 |
| 14 | [#23](https://github.com/Thatgfsj/webctl/issues/23) | [#77](https://github.com/Thatgfsj/webctl/pull/77) | 已合并 | `24fdb68` | cargo-dist 流水线 + 双语 README + 体积/冷启动基线 |

计划外但已合并的配套变更：

| PR | 内容 | merge sha |
|---|---|---|
| [#53](https://github.com/Thatgfsj/webctl/pull/53) | `.gitattributes`：测试 fixture 逐字节保真（修 Windows CRLF 改写 PDF） | `6184419` |
| [#57](https://github.com/Thatgfsj/webctl/pull/57) | 旧仓迁移映射表（`docs/MIGRATION.md`） | `7f9f76a` |
| [#58](https://github.com/Thatgfsj/webctl/pull/58) / [#61](https://github.com/Thatgfsj/webctl/pull/61) | 内容类型分支文档同步 / TESTING §6 用例登记表回填 | `f115201` / `4c958ea` |
| [#64](https://github.com/Thatgfsj/webctl/pull/64) | 修扫描器两处误报（恢复 main 绿） | `a826b9c` |
| [#67](https://github.com/Thatgfsj/webctl/pull/67) | 回环判定改为真地址 + 放宽总跨度断言余量（`127.0.0.1.evil.com` 不再被放行） | `849580f` |

**合并后 main 的本地复核**（Lead 每次合并后执行，因为"各 PR 分支绿 ≠ 合并后绿"）：
`cargo fmt --check` 通过、`clippy -D warnings` 0 问题、**241 用例全绿**（含 PR 03 的 parse 25 条）、
`cargo test --features offline` 全绿。CI 三平台（ubuntu/macos/windows）绿。

### 5.2.2 发布记录

| 版本 | tag / release | 日期 | 目标 | 备注 |
|---|---|---|---|---|
| v0.1.0 | [https://github.com/Thatgfsj/webctl/releases/tag/v0.1.0](https://github.com/Thatgfsj/webctl/releases/tag/v0.1.0) | 2026-10-17 | `x86_64-unknown-linux-gnu`、`aarch64-apple-darwin`、`x86_64-apple-darwin`、`x86_64-pc-windows-msvc` | 首个发布：`webctl` 与 `webctl-mcp` 各 4 平台资产 + shell/PowerShell 安装器；下载 + `sha256sum -c` 校验 + 干净环境 `caps --json` 实测通过（见 ACCEPTANCE §7）。**musl 静态暂缓**（[#90](https://github.com/Thatgfsj/webctl/issues/90)） |

### 5.3 验收锚点（v0.4 重定）

v0.3 的锚点（JS 页必须拿到 10 条）**作废**。v0.4 的锚点：

1. **诚实降级锚点**：`quotes.toscrape.com/js/` 必须返回 `code=DEGRADED`（或 `UNSUPPORTED`）并说明原因，**不得**返回"成功但 0 条"。
2. **静态等价锚点**：`quotes.toscrape.com/`（静态版）必须稳定取到 **10 条**（SURVEY §2.4 实测基线）。
3. **会话锚点**：本地表单站「登录→翻页→提交」端到端通过。

---

## 6. 工作流规范

### 6.1 现存的真实条款（必须遵守）

| 条款 | 出处 |
|---|---|
| PR 必须关联 issue，且先在 issue 领取，否则不通过 | dsh-ops `docs/CONTRIBUTING.md` |
| feat 前先写 issue，维护团队批准后再动手 | 同上 |
| 联动层单开逻辑区域，与核心完全解耦 | 同上 |
| 拆分独立变更；修复落在引入它的那个 PR | DSH `AGENTS.md` |
| **一次 PR 只加一类**变更，禁止"顺手规范化" | `dsh-ecosystem-spec/CONTRIBUTING.md` §4 |
| 角色不独立演进就不预先拆包 | DSH `AGENTS.md`（capability seam） |
| 业务规则独立成包；新行为走插件而非改主干 | DSH `packages/AGENTS.md` |

### 6.2 待写入的新规则（已落地）

「每 1000 行 PR 需审批」此前**没有仓库依据**（SURVEY §1.2）。措辞已写入 [CONTRIBUTING.md](../CONTRIBUTING.md) §2，本计划按它执行。两点风险仍然成立：组织级门禁是**加权 2 分制**（与行数无关）；策略类改动**合并进默认分支后才生效**。

---

## 7. 验收门禁

| 阶段 | 门禁命令 | 通过标准 |
|---|---|---|
| 每 PR | `cargo fmt --check && cargo clippy -- -D warnings && cargo test` | 全绿 |
| L0 离线 lane | `cargo test --features offline`（fixture 站 + mock HTTP，无外网） | 取回/解析/会话/文档用例全绿 |
| 真站 smoke | `./tests/e2e/run-real.sh`（可跳过的网络用例） | 静态锚点 10 条；JS 页 `DEGRADED` |
| 体积基线 | `cargo build --release --target x86_64-unknown-linux-musl` | P0 定基线后防回归 |
| PR 行数 | `git diff --shortstat` 对照 §5.1 | ≤1000 或已获批准 |

**"无浏览器 lane"这个门禁随 T3 一起消失**：v0.4 全部用例本来就无浏览器。

---

## 8. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 用户/agent 期待 JS 页面可用，实际 `DEGRADED` | 在 README、SKILL.md、信封里三处显式说明；失败绝不伪装成成功 |
| 纯 Rust PDF 文本层质量不足 | §9 先 spike；不够用则只承诺文本型 PDF，扫描件 `UNSUPPORTED` |
| 无浏览器指纹，被反爬拦截 | TLS 指纹（wreq）+ 合规头 + 限速 + robots；拦截即 `BLOCKED`，不承诺绕过 |
| 内存/响应体失控 | 全链路尺寸上限 + 截断标记 + 落盘路径（`LIMIT` 码） |
| 会话/状态泄漏 | 文件锁 + TTL + 权限 700 + 关停路径测试 |
| MCP 60s 超时 | 工具分片；长任务显式配置；单次调用预算写进 SKILL.md |
| 文档与实际能力脱节 | 本计划 §0 的代价清单必须随能力变化同步更新 |
| musl 静态交叉编译 + BoringSSL（C++） | 当前产物动态链接 libstdc++/libgcc；PR 14 必须专门验证静态目标，不许假设可行 |
| 构建期隐藏依赖（libclang / clang 内建头 / cmake） | **已落地文档**（README「构建」+ CONTRIBUTING §4）；实测三平台 CI 镜像自带、无需额外安装；spike 报告 §4 记录缺它们时的报错与修复 |

---

## 9. 待验证清单（不得当作已证实）

1. ~~`wreq` 在 Rust 1.98 + rustls 下的编译与特性组合~~ **已完成**：默认后端其实是 BoringSSL（不是 rustls），默认特性即可编译，首次全量 release 构建 76s，TLS 指纹仿真实测生效 —— 见 [docs/spikes/wreq.md](spikes/wreq.md)。
2. 纯 Rust PDF 文本层（`pdf-extract` / `lopdf`）在中文 PDF 上的实际质量。
3. `json_query` 自研子集 vs `jmespath` crate 的取舍。
4. 大响应（>50MB）下的内存表现与截断策略。
5. 礼貌限速在真实站点上的实际有效性（不被封 vs 太慢）。

---

## 10. 开放决策（未回复则按默认值执行）

| # | 决策 | 默认值 |
|---|---|---|
| 1 | 项目名与仓库 | `webctl`，`Thatgfsj/webctl`（已建） |
| 2 | JS 渲染 | **本期不做**；JS 页显式 `DEGRADED`，将来可作为可选内置引擎（Boa/rquickjs + 自建 DOM）加入 |
| 3 | OCR | 不支持；扫描件 `UNSUPPORTED` |
| 4 | 旧 Python 版 | 归档不兼容，只出迁移映射表 |
| 5 | 1000 行规则 | 按 §6.2 执行（已写入 CONTRIBUTING） |
| 6 | 许可证 | MIT（沿用） |
