# webctl 测试用例清单（TEST-CASES）

| 项 | 值 |
|---|---|
| 版本 | **v2（2026-10-16）**，对应 [PLAN v0.4](PLAN.md) |
| 状态 | **回填中**：用例先冻结，结果随实现回填。已回填 PR 01/02/04/07/08（见 §7）；PR 03/10/13 待其 PR 合并后回填 |
| 依据 | [PLAN §2/§5/§7](PLAN.md)、[SURVEY §1.4/§2.4/§2.5](SURVEY.md)、[议题 #30](https://github.com/Thatgfsj/webctl/issues/30) |
| 用例总数 | **201**（17 个域，编号唯一，见 §2） |
| v1 → v2 | v1 是 v0.3 时代（含 T2/T3）的草稿，**随方向变更整体重写**：删除 CDP / 浏览器 / 外部 CLI 全部用例，新增会话状态、抽取引擎、文档内置、礼貌限速、降级语义、资源并发等域 |

## 0. 怎么用这份清单

1. **开工前**：在 PR 议题里勾出本 PR 负责的用例（对照 §3「每 PR 最小测试交付」）。
2. **通道决定 lane**：L1/L2 不进每次 PR 的 CI，但必须在 PR 描述里贴出真实执行结果。
3. **期望必须可机械判定**：「看起来正常」不算通过；每条期望都应能写成一个 assert。
4. **失败即阻塞**：用例失败 = PR 不通过；放宽期望要单独开议题。
5. **不写假绿**：没有实现的用例不许用空测试凑数。

## 1. 执行通道与通用断言

> **2026-10-24 政策调整**：PR 阶段只跑轻量作业（`static` + `guards`），三平台 `clippy`/`test` 与 `musl-static` 只在 push main / 手动触发时跑（发布 tag 不触发 `ci.yml`）—— 覆盖面减少已如实登记在 [CI.md §8](CI.md)。

v0.4 **没有浏览器**，因此也不再有"无浏览器 lane / mock-cdp lane"之分。

| 通道 | 含义 | 命令 |
|---|---|---|
| **L0** | 离线：fixture HTTP 服务 + 本地文件，**不访问外网** | `cargo test --features offline` |
| **L1** | 真站 smoke（需要网络，可跳过） | `./tests/e2e/run-real.sh` |
| **L2** | 基线：发布物体积 / 静态性 | `cargo build --release`（glibc）；musl 由 CI `musl-static` 作业构建并用 `file`/`ldd`/`readelf` 断言静态（[#90](https://github.com/Thatgfsj/webctl/issues/90)） |
| **L3** | 安全负面：在**默认配置**下跑（不加任何放宽开关） | 并入 L0/L1 |
| **L4** | 解析/抽取**性能基线**：相对基线 + 宽 tolerance，抓**量级级退化**（正是单测 30s 挂死守卫抓不到的 `[~5s, 30s)` 档，议题 [#83](https://github.com/Thatgfsj/webctl/issues/83)） | `scripts/perf-baseline.sh --check` |

> **这些通道在 CI 上怎么跑**：见 [CI.md §2](CI.md)（L0/L2/L3 有 CI 步骤，L1/L4 是本地手动通道——**CI 绿不等于真站也绿**）。

**通用断言（每条用例都隐含，不再逐行重复）**

| 编号 | 断言 |
|---|---|
| A1 | 所有返回走信封 `{ok, data, error, code, meta}`；失败时 `data` 为空、`error` 非空 |
| A2 | 页面内容一律 `meta.trust == "untrusted_page_content"` |
| A3 | **禁止静默降级**：任何降级必须 `code != OK` 且 `meta.degraded_from` 非空 |
| A4 | 不泄漏凭据：信封与日志中无 cookie 明文、无 `Authorization`、无原始堆栈 |
| A5 | `code` 取自封闭枚举：`OK / TIMEOUT / NETWORK / BLOCKED / BAD_URL / PARSE / DEGRADED / UNSUPPORTED / SESSION_LOST / LIMIT / INTERNAL` |
| A6 | **失败可区分**：空结果 ≠ 解析失败 ≠ 能力不支持 ≠ 被拦截 ≠ 超限，五者码不同 |
| A7 | `meta.elapsed_ms` 非负，且与实测墙钟同量级（±50%） |
| A8 | **不启动任何子进程**（测试期间进程计数不增加） |

---

## 2. 用例

### 2.1 ENV — 结果信封与错误码（12）

前置：任意可用命令；L0 即可。

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| ENV-01 | 成功信封 | `ok=true`、`code="OK"`、`error=""`、`meta` 字段齐全 | L0 | PR 01 |
| ENV-02 | 失败信封 | 非法输入 → `ok=false`、`code="BAD_URL"`、`data` 为空 | L0 | PR 01 |
| ENV-03 | trust 标记 | 返回页面内容的调用 `meta.trust=="untrusted_page_content"` | L0 | PR 01 |
| ENV-04 | 禁止静默降级 | 人为触发降级 → `code!="OK"` 且 `meta.degraded_from` 非空；**不允许** `ok=true` 而 data 空 | L0 | PR 01 |
| ENV-05 | 错误码封闭集 | 遍历所有失败用例，`code` ∈ 11 个枚举值；构造未知码反序列化必须失败 | L0 | PR 01 |
| ENV-06 | 映射唯一 | 同一异常在 CLI 与 MCP 两条路径得到同一 `code` | L0 | PR 11 |
| ENV-07 | schema 快照 | golden 快照比对字段名与类型；新增字段需显式更新快照（#115 起新增 `degraded_actionable` 样本，冻结 `meta.actionable`） | L0 | PR 01 / #115 |
| ENV-08 | 未知字段容忍 | 含未知字段的信封反序列化不失败 | L0 | PR 01 |
| ENV-09 | 无堆栈泄漏 | 正则断言 `error` 不含 `panicked` / 文件路径栈帧 | L0 | PR 12 |
| ENV-10 | 空 vs 失败 | 解析零命中 `ok=true`；解析失败 `ok=false, code="PARSE"` | L0 | PR 03 |
| ENV-11 | engine 字段 | `meta.engine=="builtin"`；未定义取值序列化失败 | L0 | PR 01 |
| ENV-12 | 无子进程 | 跑完整套 L0 用例，进程计数不增加（A8 的全局版） | L0 | PR 13 |

### 2.2 CAPS — 能力自检（8）

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| CAPS-01 | 能力清单 | `caps --json` 列出 5 个能力域及其可用状态 | L0 | PR 01 |
| CAPS-02 | 无外部依赖 | 输出中**不含**任何外部工具（pdftotext/jq/pandoc/浏览器）的状态 | L0 | PR 01 |
| CAPS-03 | 版本信息 | 输出 webctl 版本与目标三元组 | L0 | PR 01 |
| CAPS-04 | 输出等价 | `--json` 与人类可读输出信息量一致 | L0 | PR 01 |
| CAPS-05 | 不联网 | `caps` 执行期间零网络请求（fixture 服务端计数为 0） | L0 | PR 01 |
| CAPS-06 | 不启动子进程 | `caps` 执行期间进程计数不变 | L0 | PR 13 |
| CAPS-07 | 退出码 | `caps` 成功退出码 0；失败时映射到错误码 | L0 | PR 10 |
| CAPS-08 | 能力开关 | 禁用某能力域后 `caps` 反映为 disabled，调用时返回 `UNSUPPORTED` | L0 | PR 10 |

### 2.3 HTTP — 取回（20）

前置：本地 fixture HTTP 服务（可注入编码/压缩/重定向/延迟/错误）。

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| HTTP-01 | UTF-8 | body 字节与 fixture 完全一致 | L0 | PR 02 |
| HTTP-02 | GBK | 解码后中文与预期字符串相等 | L0 | PR 02 |
| HTTP-03 | Shift_JIS | 同上（日文） | L0 | PR 02 |
| HTTP-04 | 无 charset 声明 | 有确定回退策略，且 lang 元数据可用 | L0 | PR 02 |
| HTTP-05 | gzip | 解压后与原文一致 | L0 | PR 02 |
| HTTP-06 | brotli | 同上 | L0 | PR 02 |
| HTTP-07 | 多跳重定向 | 链长 = 3，逐跳 URL/状态码可见 | L0 | PR 02 |
| HTTP-08 | 相对 Location | 解析为绝对 URL 且正确 | L0 | PR 02 |
| HTTP-09 | 重定向环 | 到达 max-hops 即停，返回明确错误，不无限循环 | L0 | PR 02 |
| HTTP-10 | cookie jar | 第二次请求带上第一次的 `Set-Cookie` | L0 | PR 02 |
| HTTP-11 | cookie 属性 | `Domain`/`Path`/`Secure`/`HttpOnly` 生效（越界不下发） | L0 | PR 02 |
| HTTP-12 | 超时 | 服务端 sleep > 超时 → `TIMEOUT` | L0 | PR 02 |
| HTTP-13 | 连接失败 | 关闭端口 → `NETWORK` | L0 | PR 02 |
| HTTP-14 | 4xx/5xx | 归类为 `BLOCKED`，且与 `PARSE` 不混用 | L0 | PR 02 |
| HTTP-15 | 内容类型路由 | html/json/xml/pdf/binary 各命中分支（5 断言） | L0 | PR 02 |
| HTTP-16 | 非法 URL | `BAD_URL`，服务端请求计数为 0 | L0 | PR 02 |
| HTTP-17 | 大响应 | 50MB 响应不 OOM；截断时带截断标记且 `code="LIMIT"` | L0 | PR 02 |
| HTTP-18 | 二进制落盘 | 二进制类型不当作文本解析，走落盘路径 | L0 | PR 02 |
| HTTP-19 | 请求头策略 | 默认 UA/头可配置，且不发送自相矛盾的头 | L0 | PR 08 |
| HTTP-20 | 无 shell | 整个取回路径不经过任何外部命令（进程计数） | L0 | PR 13 |
| HTTP-21 | 请求预算 | 含重试在内的总请求预算不超过配置上限（防放大） | L0 | PR 08 |

### 2.4 PARSE — 解析（16）

前置：离线 fixture 站（静态 HTML 集合，含畸形与多语言样本）。

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| PARSE-01 | 正文提取 | 含正文首尾句，不含导航/页脚文案 | L0 | PR 03 |
| PARSE-02 | 噪声过滤 | 侧栏关键词不出现在正文 | L0 | PR 03 |
| PARSE-03 | 畸形 HTML | 未闭合标签仍可提取且不 panic | L0 | PR 03 |
| PARSE-04 | 编码联合 | GBK 页面正文无乱码（与 HTTP-02 联动） | L0 | PR 03 |
| PARSE-05 | 链接抽取 | 数量/绝对化/rel 全对 | L0 | PR 03 |
| PARSE-06 | base href | 相对链接按 `<base>` 解析 | L0 | PR 03 |
| PARSE-07 | 表格抽取 | 行列数与逐格文本相等 | L0 | PR 03 |
| PARSE-08 | 跨行跨列 | colspan/rowspan 语义确定，不丢格 | L0 | PR 03 |
| PARSE-09 | 表单抽取 | 字段名/类型/默认值/action/method 全对 | L0 | PR 03 |
| PARSE-10 | markdown 输出 | 标题/列表/链接渲染为对应标记 | L0 | PR 03 |
| PARSE-11 | JSON/XML | 结构化解析并保留原始文本可回溯 | L0 | PR 03 |
| PARSE-12 | 选择器多命中 | 返回数组且顺序与文档序一致 | L0 | PR 06 |
| PARSE-13 | 零命中 vs 失败 | 见 ENV-10，两条路径码不同 | L0 | PR 03 |
| PARSE-14 | 解析预算 | 100KB HTML 解析**不挂死**（挂死守卫 30s，非性能门禁）+ 耗时计入 `elapsed_ms` 且与实测同量级 | L0 | PR 03 |
| PARSE-15 | 非 HTML 输入 | 明确 `UNSUPPORTED` 或路由到正确处理器，不 panic | L0 | PR 03 |
| PARSE-16 | 深度防御 | 极深嵌套（>1000 层）不栈溢出 | L0 | PR 03 |

### 2.5 SESS — 协议级会话引擎（14）

前置：本地 axum/axum-like 表单站（登录 / 分页 / 提交 / 上传 / SPA 页 / 会轮换的 CSRF）。

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| SESS-01 | 登录成功 | 登录后受保护页面 200 且内容匹配 | L0 | PR 04 |
| SESS-02 | 未登录 | 受保护页面被拒，返回可区分的错误码 | L0 | PR 04 |
| SESS-03 | cookie 持久化 | 第二次调用无需重新登录（跨进程） | L0 | PR 04 |
| SESS-04 | CSRF 回填 | 提交后服务端断言 token 匹配 | L0 | PR 04 |
| SESS-05 | CSRF 轮换 | token 变化后仍提交成功 | L0 | PR 04 |
| SESS-06 | 隐藏字段 | 服务端收到的隐藏字段与页面一致 | L0 | PR 04 |
| SESS-07 | multipart 上传 | 服务端收到文件且字节数一致 | L0 | PR 04 |
| SESS-08 | 分页遍历 | 各页条数之和 = 站点总数；有页数上限 | L0 | PR 04 |
| SESS-09 | 分页终止 | 循环 next（A→B→A）时在 max-pages 内停止并标记 `LIMIT` | L0 | PR 04 |
| SESS-10 | SPA 降级 | `code="DEGRADED"` 且 `meta.degraded_from` 非空 | L0 | PR 09 |
| SESS-11 | 登录态导出/导入 | 导出→新会话导入→受保护页可访问 | L0 | PR 04 |
| SESS-12 | 非法会话名 | `../` 等路径穿越被拒，不落到 state 目录外 | L0+L3 | PR 05 |
| SESS-13 | 重定向登录 | 登录后 302 回跳原页面，会话保持 | L0 | PR 04 |
| SESS-14 | 会话并发 | 同一会话被两个请求并发使用时结果正确（无串号） | L0 | PR 05 |

### 2.6 STATE — 会话与状态持久化（10）

前置：可控制权限与磁盘状态的临时 state dir。

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| STATE-01 | 目录权限 | state dir 权限 700，文件 600 | L0+L3 | PR 05 |
| STATE-02 | 文件锁 | 两进程同时写同一会话：不损坏；后到者等待或明确报错 | L0 | PR 05 |
| STATE-03 | 锁超时 | 持锁进程被杀后，新进程在超时内获得锁（无死锁） | L0 | PR 05 |
| STATE-04 | TTL 回收 | 过期会话不可用且返回可读错误 | L0 | PR 05 |
| STATE-05 | 损坏文件 | 截断/乱码的会话文件 → 明确错误码，**不静默重建** | L0 | PR 05 |
| STATE-06 | CLI/MCP 共享 | CLI 写入的会话 MCP 可见，反之亦然 | L0 | PR 11 |
| STATE-07 | 路径约束 | 会话名不能逃出 state dir（含 symlink 越界） | L0+L3 | PR 05 |
| STATE-08 | 原子写 | 写入过程被中断（模拟）后文件仍为旧值或新值，不出现半截 | L0 | PR 05 |
| STATE-09 | 清理命令 | 清理只删自己的会话，不影响其他会话 | L0 | PR 05 |
| STATE-10 | 无凭据泄漏 | 会话文件权限与内容都不含明文密码（只存 cookie/令牌） | L0+L3 | PR 12 |

### 2.7 EXTRACT — 抽取与选择器（17）

前置：结构复杂的 fixture 站（嵌套、重复、动态 class、shadow-free）。

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| EXTRACT-01 | 结构化路径 | 每条命中带 tag / 同级索引 / 祖先链 | L0 | PR 06 |
| EXTRACT-02 | 上下文 | 命中附文本上下文（前后若干字符） | L0 | PR 06 |
| EXTRACT-03 | 多命中保序 | 顺序与文档序一致 | L0 | PR 06 |
| EXTRACT-04 | 字符预算 | 超预算时截断且标记，`code="LIMIT"` | L0 | PR 06 |
| EXTRACT-05 | 命中数上限 | 超过 max-results 时截断并标记 | L0 | PR 06 |
| EXTRACT-06 | 零命中 | `ok=true`，空数组 + 可读结论 | L0 | PR 06 |
| EXTRACT-07 | 非法选择器 | `PARSE` 错误，不 panic、不返回空当成功 | L0 | PR 06 |
| EXTRACT-08 | search 文本 | 命中数/位置/上下文正确 | L0 | PR 06 |
| EXTRACT-09 | search 正则 | 非法正则 → 明确错误 | L0 | PR 06 |
| EXTRACT-10 | tables 结构化 | 表头/行/单元格与 fixture 全等 | L0 | PR 06 |
| EXTRACT-11 | forms 输出 | 字段清单可直接喂给 `fill_form` | L0 | PR 06 |
| EXTRACT-12 | links 过滤 | 按域名/rel/正则过滤生效 | L0 | PR 06 |
| EXTRACT-13 | 稳定性语义 | 文档写明"选择器跨请求不保证命中同一元素"，并有用例演示差异 | L0 | PR 06 |
| EXTRACT-14 | 大页面 | 1MB HTML 抽取不超时、结果有界 | L0 | PR 06 |
| EXTRACT-15 | 与解析一致 | 同一 fixture 下 `extract` 结果与 `PARSE-*` 断言一致 | L0 | PR 06 |
| EXTRACT-16 | 不落原文 | 结果里不夹带无法审计的二进制/原始字节 | L0 | PR 06 |
| EXTRACT-17 | 灾难性输入 | 超长/恶意选择器不导致 CPU 爆炸（有超时与复杂度上限） | L0 | PR 06 |

### 2.8 FORM — 表单填写与提交（12）

前置：多字段表单站（文本 / 下拉 / 复选 / 单选 / 隐藏 / 文件 / 多表单页面）。

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| FORM-01 | 单字段填值 | 服务端收到该字段的值 | L0 | PR 04 |
| FORM-02 | 一次填多字段 | `fill_form` 一次调用完成全部字段 | L0 | PR 04 |
| FORM-03 | 中文值 | 中文正确编码传输（服务端断言字节） | L0 | PR 04 |
| FORM-04 | select | 选中项与提交值一致 | L0 | PR 04 |
| FORM-05 | checkbox/radio | 勾选与不勾选两种行为都符合 HTML 语义 | L0 | PR 04 |
| FORM-06 | 隐藏字段 | 未被覆盖时保留原值 | L0 | PR 04 |
| FORM-07 | 未知字段 | 提交不存在的字段 → 明确错误（不静默忽略） | L0 | PR 04 |
| FORM-08 | 缺必填 | 缺必填字段时给出可读错误（或按服务端响应归类） | L0 | PR 04 |
| FORM-09 | 多表单页面 | 用 `form_index` 正确选定表单 | L0 | PR 06 |
| FORM-10 | 文件字段 | 文件上传与字段一起提交成功 | L0 | PR 04 |
| FORM-11 | 提交后落点 | 提交后返回最终 URL 与状态可追溯 | L0 | PR 04 |
| FORM-12 | 幂等保护 | 明显重复提交（同一 token 复用）被显式拒绝或报告 | L0 | PR 04 |

### 2.9 DOC — 文档与数据内置处理（12）

前置：本地 PDF 样本（文本型 / 中文 / 扫描件）、JSON 样本。

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| DOC-01 | PDF 文本层 | 提取文本含预期关键词 | L0 | PR 07 |
| DOC-02 | 中文 PDF | 中文不乱码、段落顺序合理 | L0 | PR 07 |
| DOC-03 | 扫描件 | `UNSUPPORTED`，**不得**返回空文本当成功 | L0 | PR 07 |
| DOC-04 | 加密 PDF | 明确错误（不 hang、不崩） | L0 | PR 07 |
| DOC-05 | 损坏 PDF | `PARSE` 或明确错误码 | L0 | PR 07 |
| DOC-06 | json_query 路径 | 路径取值与预期相等 | L0 | PR 07 |
| DOC-07 | json_query 过滤/投影 | 过滤与投影语义与文档一致 | L0 | PR 07 |
| DOC-08 | json_query 非法表达式 | 明确错误，不返回 null 当成功 | L0 | PR 07 |
| DOC-09 | HTML→文本 | 复用解析层，不重复实现（结果与 PARSE-01 一致） | L0 | PR 07 |
| DOC-10 | 大文件上限 | 超限 → `LIMIT`，不 OOM | L0 | PR 07 |
| DOC-11 | 未知格式 | `UNSUPPORTED` + 可读诊断 | L0 | PR 07 |
| DOC-12 | 无外部进程 | 文档处理全程零子进程（A8） | L0 | PR 13 |

### 2.10 POLITE — 礼貌与限速（10）

前置：fixture 服务端可统计请求时间戳与并发数。

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| POLITE-01 | 并发上限 | 每主机并发 ≤ 2（服务端观测到的最大并发） | L0 | PR 08 |
| POLITE-02 | 最小间隔 | 同主机请求间隔 ≥ 配置值 | L0 | PR 08 |
| POLITE-03 | 可配置 | 修改配置后上限/间隔随之变化 | L0 | PR 08 |
| POLITE-04 | Retry-After | 429 + `Retry-After: 2` → 等待后再试 | L0 | PR 08 |
| POLITE-05 | 退避上限 | 连续失败时退避有上限，不无限重试 | L0 | PR 08 |
| POLITE-06 | robots 允许 | robots.txt 允许时正常取回 | L0 | PR 08 |
| POLITE-07 | robots 禁止 | 被禁止路径 → 明确拒绝（不偷偷抓） | L0 | PR 08 |
| POLITE-08 | robots 关闭 | 显式关闭时在信封里记录"已关闭" | L0 | PR 08 |
| POLITE-09 | 拦截页 | Cloudflare/验证码特征页 → `BLOCKED` + 诊断 | L0 | PR 08 |
| POLITE-10 | 不轰炸 | 被拦截后不进入高频重试（请求数有上界断言） | L0 | PR 08 |

### 2.11 DEGRADE — JS/SPA 降级语义（10）

前置：JS 页样本（本地 fixture 模拟 + 真实站）。

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| DEGRADE-01 | JS 页锚点 | `quotes.toscrape.com/js/` → `code="DEGRADED"` 或 `"UNSUPPORTED"` | L1 | PR 09 |
| DEGRADE-02 | 静态对照 | `quotes.toscrape.com/` 稳定取到 **10 条** | L1 | PR 09 |
| DEGRADE-03 | 诊断可解释 | `error` 说明判定依据（脚本密度/空容器/noscript/框架指纹） | L0 | PR 09 |
| DEGRADE-04 | noscript 页面 | 仅 `<noscript>` 有内容 → `DEGRADED` | L0 | PR 09 |
| DEGRADE-05 | 空容器 + 脚本 | 主容器为空且脚本多 → `DEGRADED` | L0 | PR 09 |
| DEGRADE-06 | 不误判 | 静态但脚本多的页面**不得**判为 DEGRADED | L0 | PR 09 |
| DEGRADE-07 | 强制开关 | 跳过判定返回原始 HTML，且 `meta.degraded_from` 非空 | L0 | PR 09 |
| DEGRADE-08 | 禁止"成功但 0 条" | 需 JS 的页面零命中时，`code` 必须非 OK | L0 | PR 09 |
| DEGRADE-09 | SPA 空壳 | 首屏空壳 → `UNSUPPORTED`，`data` 说明为何无内容 | L0 | PR 09 |
| DEGRADE-10 | 不编造内容 | 不得用启发式/模型"补全"未渲染的内容（代码审计 + 用例） | L0 | PR 09 |

### 2.12 CLI — 命令行与配置（12）

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| CLI-01 | 配置优先级 | CLI 参数 > 环境变量 > 配置文件 > 默认（4 层断言） | L0 | PR 10 |
| CLI-02 | 配置文件错误 | 语法错误 → 明确报错与非零退出 | L0 | PR 10 |
| CLI-03 | 代理 | 配置文件/环境变量代理生效（fixture 代理计数） | L0 | PR 10 |
| CLI-04 | 输出格式 | `json` / `text` 切换正确；json 可被解析器消费 | L0 | PR 10 |
| CLI-05 | 退出码 | 错误码 → 退出码映射稳定且文档化 | L0 | PR 10 |
| CLI-06 | offline | `--offline` 下零网络访问（fixture 计数为 0） | L0 | PR 10 |
| CLI-07 | 帮助 | `--help` 列出全部子命令且与文档一致 | L0 | PR 10 |
| CLI-08 | 未知参数 | 非零退出 + 可读提示 | L0 | PR 10 |
| CLI-13 | 浏览器上下文 CLI 开关 | `--browser-context` 注入 UA/accept-language/cookie；`--no-browser-context` 压过环境变量；两开关互斥（退出码 2）；坏文件**失败**不静默（[#183](https://github.com/Thatgfsj/webctl/issues/183)） | L0 | 0.2.1 |
| CLI-09 | 管道友好 | 单行 JSON 输出可直接管道给下游工具 | L0 | PR 10 |
| CLI-10 | 无凭据输出 | 任何输出/日志不含 cookie/令牌（正则） | L0+L3 | PR 12 |
| CLI-11 | 环境变量文档 | 代码使用的环境变量全部在文档中列出 | L0 | PR 10 |
| CLI-12 | 启动开销 | 冷启动（`--version`）< 50ms，不加载网络栈 | L2 | PR 14 |

### 2.13 MCP — 工具面与 DSH 契约（14）

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| MCP-01 | 握手 | `initialize` 返回协商后的 protocolVersion | L0 | PR 11 |
| MCP-02 | 工具清单 | `tools/list` 恰好 **16** 个工具且名字与 `TOOL_NAMES`/PLAN §3.7 一致（#114 起） | L0 | PR 11 / #114 |
| MCP-03 | schema 合法 | 每个工具 `inputSchema` 为合法 JSON Schema（schemars 生成） | L0 | PR 11 |
| MCP-04 | 命名契约 | 挂载后工具名为 `mcp__webctl__<rawName>`，满足 function-name 契约 | L0 | PR 11 |
| MCP-05 | 指令上限 | 服务器指令 ≤ **32768** 字节 | L0 | PR 11 |
| MCP-06 | 二次启动 | 服务进程可被启动两次而不冲突（stdio 探测场景） | L0 | PR 11 |
| MCP-07 | 会话归属 | 跨 Session 调用被拒，错误可读 | L0 | PR 11 |
| MCP-08 | toolOrder | 配置中不写入 webctl 工具名（断言配置文件） | L0 | PR 11 |
| MCP-09 | 调用预算 | 单次工具调用 < 60s 返回（长任务有分片说明） | L1 | PR 11 |
| MCP-10 | 错误信封 | 失败以信封返回，而非 JSON-RPC 协议错误 | L0 | PR 11 |
| MCP-11 | 大结果 | 大结果落盘返回路径，不无脑内联 | L0 | PR 11 |
| MCP-12 | SKILL.md 一致 | `SKILL.md` 声明的工具与 `tools/list` 一致（有校验脚本） | L0 | PR 11 |
| MCP-13 | 状态共享 | MCP 与 CLI 共享 state dir（见 STATE-06） | L0 | PR 11 |
| MCP-14 | 零外部进程 | MCP 运行期无子进程、无浏览器（进程计数） | L0 | PR 13 |

### 2.14 SEC — 安全与凭据（15）

前置：**默认配置**（不加任何放宽开关）；L3。

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| SEC-01 | 默认严格 | 默认配置下不存在任何"放宽"开关生效 | L3 | PR 12 |
| SEC-02 | 导出需确认 | `cookies` 导出无 `--yes` 时被拒 | L3 | PR 12 |
| SEC-03 | 导出权限 | 导出文件权限 600 | L3 | PR 12 |
| SEC-04 | 信封脱敏 | 信封中 cookie 值被脱敏 | L3 | PR 12 |
| SEC-05 | 日志脱敏 | 日志中 cookie 值被脱敏 | L3 | PR 12 |
| SEC-06 | Authorization | 跨域重定向时不泄漏 `Authorization` | L3 | PR 12 |
| SEC-07 | query token | URL query 中的 token 在日志中脱敏 | L3 | PR 12 |
| SEC-08 | 密码字段 | 表单密码字段值不落日志 | L3 | PR 12 |
| SEC-09 | trust 标记 | MCP 路径同样标 `untrusted_page_content` | L3 | PR 11 |
| SEC-10 | 无 shell | 代码中无子进程调用（静态检查）+ 运行时进程计数不变 | L3 | PR 13 |
| SEC-11 | 路径穿越 | 所有文件参数经 canonicalize，越界被拒 | L3 | PR 12 |
| SEC-12 | 状态权限 | state dir 700、文件 600（与 STATE-01 双向验证） | L3 | PR 05 |
| SEC-13 | 无堆栈 | 错误输出不含堆栈/文件路径 | L3 | PR 12 |
| SEC-14 | 下载白名单 | 下载类型白名单与大小上限生效 | L3 | PR 12 |
| SEC-15 | 危险协议 | `file://` 等非 http(s) 协议被拒 | L3 | PR 12 |

### 2.15 RES — 资源与并发（10）

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| RES-01 | 全局并发 | 总并发受配置约束，服务端观测值 ≤ 上限 | L0 | PR 08 |
| RES-02 | 取消释放 | 取消/超时后连接与任务被释放（句柄计数回落） | L0 | PR 02 |
| RES-03 | 连续请求 | 连续 1000 次请求 RSS 增长有界（< 阈值） | L0 | PR 13 |
| RES-04 | 大响应内存 | 50MB 响应路径内存有界（截断/流式） | L0 | PR 02 |
| RES-05 | 句柄泄漏 | 长跑后 fd 数回到基线 | L0 | PR 13 |
| RES-06 | 长跑 | 30 分钟混合负载无内存/fd 泄漏（可夜间跑） | L1 | PR 13 |
| RES-07 | 错误路径不崩 | 全部错误用例不 panic、不 abort | L0 | PR 13 |
| RES-08 | 无忙等 | 空闲时 CPU < 1% | L0 | PR 13 |
| RES-09 | 队列有界 | 请求队列有上限，超限时背压/拒绝而非无限排队 | L0 | PR 02 |
| RES-10 | 无子进程泄漏 | 全流程零子进程创建（A8 的压测版） | L0 | PR 13 |

### 2.16 PKG — 打包与发布（8）

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| PKG-01 | release 构建 | `cargo build --release` 成功 | L2 | PR 14 |
| PKG-02 | musl 静态 | **已达成（v0.1.1 发布资产）**：`webctl-x86_64-unknown-linux-musl.tar.xz` 可运行（`--version` / `caps --json`），并已在**真实 Alpine(musl)** 里跑通 `fetch` 与 MCP `extract`；CI 的 `musl-static` 作业在 push main 时（PR 阶段不跑，见 CI.md §8） 同样断言静态 | L2 | #90（已收口） |
| PKG-03 | 三平台 | linux/macos/windows 资产齐全 | L2 | PR 14 |
| PKG-04 | 版本 | `--version` 与 tag 一致 | L2 | PR 14 |
| PKG-05 | 体积基线 | 不超过 P0 定下的基线 | L2 | PR 14 |
| PKG-06 | 干净环境 | 无 Rust/无浏览器的干净环境可直接运行 | L2 | PR 14 |
| PKG-07 | 文档一致 | README 安装命令与实际资产一致（无虚构命令） | L2 | PR 14 |
| PKG-08 | 无动态依赖 | **已达成（v0.1.1）**：musl 资产 `ldd` = `statically linked`、`readelf -d` 无 NEEDED，并已在**真实 Alpine(musl)** 里跑通；glibc 资产为动态链接（预期）；CI 的 `musl-static` 作业在 push main 时（PR 阶段不跑，见 CI.md §8） 同样断言 | L2 | #90（已收口） |

### 2.17 E2E — 真实站点锚点（7）

前置：需要网络，可跳过；**只跑符合目标站 robots 与使用条款的用例**。

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| E2E-01 | 静态锚点 | `quotes.toscrape.com/` 取到 10 条 | L1 | PR 09 |
| E2E-02 | 降级锚点 | `quotes.toscrape.com/js/` → `DEGRADED`（不是 0 条成功） | L1 | PR 09 |
| E2E-03 | 真实表单流 | 真实站点上一轮"取回→解析→填表→提交"闭环 | L1 | PR 04 |
| E2E-04 | robots 遵守 | 真实站点 robots 禁止路径被拒 | L1 | PR 08 |
| E2E-05 | PDF 真实样本 | 公开文本型 PDF 提取成功 | L1 | PR 07 |
| E2E-06 | 长跑 | 30 分钟混合负载无泄漏（可与 RES-06 合并执行） | L1 | PR 13 |
| E2E-07 | 拦截识别 | 命中拦截页时归类 `BLOCKED` 且不重试轰炸 | L1 | PR 08 |

---

## 3. 每 PR 最小测试交付（本清单的用法）

| PR | 必须新增/通过的用例 | 不可省的三条 |
|---|---|---|
| 01 骨架 + 信封 + caps + CI | ENV-01…11、CAPS-01…05,07 | ENV-04 禁止静默降级、ENV-05 封闭集、CAPS-02 无外部依赖 |
| 02 取回 | HTTP-01…18,20、RES-02,04,09 | HTTP-02 GBK、HTTP-09 重定向环、HTTP-17 大响应 |
| 03 解析 | PARSE-01…11,13…16 | PARSE-03 畸形 HTML、PARSE-09 表单、PARSE-14 预算 |
| 04 会话引擎 | SESS-01…09,11,13,14、FORM-01…08,10…12；**额外交付 SESS-15、FORM-13…19**（见 §7.3） | SESS-04 CSRF、SESS-08 分页、FORM-02 批量填表 |
| 05 会话与状态 | SESS-12、STATE-01…05,07…09 | STATE-02 文件锁、STATE-05 损坏文件、STATE-01 权限 |
| 06 抽取引擎 | PARSE-12、EXTRACT-01…17、FORM-09 | EXTRACT-04 截断、EXTRACT-10 表格、EXTRACT-13 稳定性语义 |
| 07 文档与数据 | DOC-01…12 | DOC-03 扫描件 UNSUPPORTED、DOC-06 json 路径、DOC-10 上限 |
| 08 礼貌与限速 | HTTP-19,21、POLITE-01…10、RES-01 | POLITE-01 并发上限、POLITE-07 robots 禁止、POLITE-09 拦截识别 |
| 09 降级语义 | SESS-10、DEGRADE-01…10、E2E-01,02 | DEGRADE-01 锚点、DEGRADE-08 禁止假成功、DEGRADE-06 不误判 |
| 10 CLI 与配置 | CAPS-08、CLI-01…12 | CLI-01 配置优先级、CLI-06 offline、CLI-05 退出码 |
| 11 MCP | ENV-06、STATE-06、MCP-01…14 | MCP-05 32KB、MCP-06 二次启动、MCP-10 错误信封 |
| 12 安全与凭据 | ENV-09、STATE-10、CLI-10、SEC-01…15 | SEC-02 导出确认、SEC-06 跨域不泄漏、SEC-11 路径穿越 |
| 13 测试基建 | ENV-12、CAPS-06、HTTP-20、DOC-12、MCP-14、SEC-10、RES-03,05…10 | RES-05 fd 泄漏、RES-08 无忙等、SEC-10 无 shell |
| 14 打包发布 | PKG-01…08、CLI-12 | PKG-05 体积、PKG-06 干净环境、PKG-07 文档一致 |

## 4. 测试资产清单（议题「测试基建」的输入）

| 资产 | 内容 | 服务用例 |
|---|---|---|
| **fixture HTTP 服务** | 可注入编码（UTF-8/GBK/Shift_JIS）、压缩、重定向（含环）、延迟、4xx/5xx、大响应、429+Retry-After、robots.txt | HTTP-*、POLITE-*、RES-* |
| **fixture 表单站** | 登录、分页、CSRF 轮换、隐藏字段、文件上传、多表单、SPA 空壳页 | SESS-*、FORM-* |
| **fixture 解析站** | 文章页、噪声、畸形 HTML、多语言、深嵌套、复杂表格、JSON/XML | PARSE-* |
| **fixture 抽取站** | 重复元素、嵌套结构、动态 class、1MB 大页面 | EXTRACT-* |
| **PDF 样本集** | 文本型 / 中文 / 扫描件 / 加密 / 损坏 | DOC-* |
| **golden 目录** | 信封 schema 快照、解析/抽取期望输出 | ENV-07、PARSE-*、EXTRACT-* |
| **状态环境工具** | 临时 state dir、权限检查、并发进程编排、文件损坏注入 | STATE-*、SEC-03,12 |
| **资源观测工具** | fd 计数、RSS 采样、子进程计数、CPU 空闲采样 | RES-*、SEC-10、MCP-14 |
| **真站 smoke 脚本** | `tests/e2e/run-real.sh`，可跳过、遵守 robots | E2E-* |
| **体积基线记录** | 见 `docs/size-baseline.json`（**v0.2.0 真实发布资产实测，2026-10-26**）：glibc **34,767,656 B**（动态，p50 6ms）、musl **34,122,368 B**（静态，p50 5ms）；两者 `source=release-asset`、`commit=b2185b3`（sha256 已校验） |

## 5. 覆盖缺口（不假装能测）

| 缺口 | 原因 | 处理 |
|---|---|---|
| JS 渲染正确性 | v0.4 没有 JS 引擎 | 只测"降级是否正确"，不测"渲染是否正确"；将来引入内置引擎时补 DOM/JS 用例 |
| 真实反爬对抗 | 不承诺绕过，且对抗是持续博弈 | 只测"拦截被识别为 `BLOCKED`"，不测"能否通过" |
| 真实站点结构漂移 | 外部站点随时改版 | E2E 只做锚点与烟雾，不作回归门禁；结构性回归靠 fixture |
| 跨平台行为差异 | CI 只有 linux/macos/windows 三平台构建 | 运行期差异靠 PKG-06 干净环境抽样，不承诺全矩阵功能测试 |
| 极端大文件/OOM | CI 资源有限 | 用阈值断言模拟，真实极限留手工验证 |
| 人类可读性/文案质量 | 无法机械判定 | 由评审把关，不进本清单 |

## 6. 实现回填（真实用例登记表）

> 本节兑现表头"结果随实现回填"的承诺。**测试名取自 main 上的真实函数名**，供审阅者机械核对；
> 新增用例必须同时在此登记（见 §6 维护规则）。

### 6.1 PR 01 骨架（议题 #40）

`crates/webctl-core/` 单元测试 **13 条**：`ok_envelope_shape_and_trust`、`failure_envelope_shape`、`degradation_must_be_explicit`、`unknown_fields_are_tolerated`、`unknown_engine_is_rejected`、`reports_all_domains_in_order`、`report_never_mentions_external_tools`、`offline_disables_fetch_only`、`defaults_are_conservative`、`output_format_strings_are_stable`、`code_set_and_strings_are_frozen`、`unknown_code_is_rejected`、`ok_and_degradation_are_disjoint`。服务 ENV-01…11、CAPS-01…05,07。

### 6.2 PR 02 取回（议题 #1）

`crates/webctl-web/tests/transport.rs` **21 条**：`http_01_utf8_body_matches_fixture`、`http_02_gbk_decodes`、`http_03_shift_jis_decodes`、`http_04_missing_charset_falls_back_deterministically`、`http_05_gzip_is_decompressed`、`http_06_brotli_is_decompressed`、`http_07_three_hop_redirect_chain_is_visible`、`http_08_relative_location_is_resolved`、`http_09_redirect_loop_terminates_with_error`、`http_10_cookie_jar_replays_cookie`、`http_11_cookie_constraints_are_enforced`、`http_12_timeout_maps_to_timeout_and_releases`、`http_13_connection_failure_is_network`、`http_14_http_error_statuses_are_blocked`、`http_15_content_type_routing_hits_five_branches`、`http_16_invalid_url_never_connects`、`http_17_large_response_is_truncated_with_limit`、`http_18_binary_is_not_parsed_as_text`，外加 `cookie_snapshot_roundtrips_through_json`、`envelope_shape_and_trust_hold`、`elapsed_ms_tracks_wall_clock`。服务 HTTP-01…18,20、RES-02,04,09。

### 6.3 PR 04 会话引擎（议题 #4）

会话域**按主题拆成两个测试二进制**（#171：单文件 < 1000 行）：`crates/webctl-web/tests/session.rs` **15 条 SESS** + `crates/webctl-web/tests/session_forms.rs` **18 条**（17 FORM + 1 fixture 自检），共享 fixture/辅助在 `tests/support/session_site.rs` / `tests/support/session_helpers.rs`（用 `#[path]` 引入，不单独成为测试目标）：

- **SESS**：`sess_01_login_success`、`sess_02_not_logged_in_is_session_lost`、`sess_03_cookie_persists_across_engines`、`sess_04_csrf_is_extracted_and_refilled`、`sess_05_csrf_rotation_is_refetched`、`sess_06_hidden_fields_are_preserved`、`sess_07_form_10_multipart_upload`、`sess_08_pagination_sums_items`、`sess_09_loop_pagination_stops_with_limit`、`sess_10_spa_degrades`、`sess_11_export_import_session_state`、`sess_12_invalid_session_name_is_rejected`、`sess_13_redirect_login_keeps_session`、`sess_14_concurrent_requests_share_session`、**`sess_15_paginate_spa_stays_degraded`**（首轮审阅否决后补：分页必须传播 DEGRADED，不得吞成 OK+0 条）。
- **FORM**：`form_01_single_field`、`form_02_multiple_fields_at_once`、`form_03_unicode_value`、`form_04_select_value`、`form_05_checkbox_and_radio_semantics`、`form_06_hidden_field_default`、`form_07_unknown_field_is_error`、`form_08_missing_required`、`form_11_landing_url_and_status`、`form_12_duplicate_submission_refused`，**新增** `form_13_duplicate_checkbox_first_checked_only`、`form_14_duplicate_checkbox_second_checked_only`、`form_15_duplicate_text_inputs_keep_order`、`form_16_select_multiple_submits_all_selected`、`form_17_307_preserves_post_body`、`form_18_308_preserves_post_body`、`form_19_303_switches_to_get_without_body`（首轮审阅否决后补：同名多值与 307/308 丢 body）。
- 自检：`fixture_multipart_parser_sanity`。

### 6.4 PR 07 文档与数据（议题 #36）

`crates/webctl-web/tests/doc.rs` **14 条通过 + 0 ignored**（DOC-09 于 2026-10-17 收口，见 ACCEPTANCE §7）：`doc_01_text_pdf_extracts_keywords`、`doc_02_chinese_pdf_is_not_mojibake`、`doc_03_scanned_pdf_is_unsupported`、`doc_04_encrypted_pdf_is_explicit_error`、`doc_05_corrupt_pdf_is_parse_error`、`doc_06_json_path_selection`、`doc_07_json_filter_and_projection`、`doc_08_invalid_expression_errors`、`doc_09_acceptance_after_pr03`（DOC-09 转正）、`doc_10_oversized_input_is_limit`、`doc_11_unknown_format_is_unsupported_with_diagnostic`、`doc_pdf_extract_panic_is_isolated_as_parse`（补：pdf-extract panic 必须隔离为 PARSE）、`four_outcomes_have_distinct_codes`、`doc_plain_text_and_json_routing_are_explicit`（补：纯文本直通的内容级断言 + JSON 路由到 `json_query`，原属过渡期用例）。

### 6.5 PR 08 礼貌与限速（议题 #37）

- `crates/webctl-web/tests/politeness.rs` **8 条**：`robots_disallow_blocks_without_requesting`、`robots_can_be_disabled_and_is_recorded`、`min_interval_is_enforced`、`concurrency_is_capped`、`retry_after_is_respected`、`interception_is_blocked_without_retry_storm`、`server_error_is_blocked`、`network_errors_retry_within_budget`。
- `crates/webctl-web/src/politeness/**` 单元 **17 条**，含首轮审阅否决后补的三条回归：`queued_requests_still_keep_min_interval`（排队场景最小间隔）、`consecutive_user_agent_lines_share_one_group`、`longest_matching_agent_token_wins`。

### 6.6 历史：早期「待回填」占位清单（**已全部回填**）

> 本节是当年 PR 尚未合并时的占位清单，**保留为历史记录**。PR 03 / 10 / 13 早已合并，用例已分别登记到本文件 §6 的对应小节（§6.2 / §6.4 与 §6.7 起各节）；下表数字是**当时的快照**——例如 `offline_lane` 当时记作「4+2 ignored」，现在是 **5 条 / 0 ignored**（DOC-12 已真实化、重复的 MCP-14 占位已删除）。

| PR | 已交付用例（当时报告的快照） |
|---|---|
| 03 解析（#54） | `tests/parse.rs` 23 条，含 XML 深度预检对抗用例（本轮修 XML 栈溢出后新增） |
| 10 CLI（#60） | `cargo test` 133 passed，含 cli 集成 13 条（CLI-01…11）；CLI-12 属 PR 14 |
| 13 测试基建（#55） | **已合并（e899c2f）**：golden 2 / http_fixture 9 / offline_lane 4+2 ignored / resource 11；含三条「扫描器不空转」永久护栏 + 一条「该放行的不误报」护栏（`scanner_does_not_flag_own_binary_or_loopback_userinfo`） |

### 6.7 PR 06 抽取引擎（议题 #35）

\`crates/webctl-web/tests/extract.rs\` **26 条**（离线；只读本地 fixture 与内存字符串）：

- **PARSE-12**：\`parse_12_full_extract_semantics\`
- **EXTRACT-01…17**：\`extract_01_structured_path\`、\`extract_02_context\`、\`extract_03_document_order\`、\`extract_04_char_budget_truncates_and_marks\`、\`extract_05_max_results_truncates_and_marks\`、\`extract_06_zero_hits_is_ok_empty\`、\`extract_07_invalid_selector_is_parse\`、\`extract_08_search_text_positions_and_context\`、\`extract_09_search_regex_and_invalid_pattern\`、\`extract_10_tables_match_fixture\`、\`extract_11_forms_feed_fill_form\`、\`extract_12_link_filters\`、\`extract_13_stability_semantics\`、\`extract_14_large_page_is_bounded\`、\`extract_15_matches_parse_layer\`、\`extract_16_no_raw_bytes\`、\`extract_17_catastrophic_input_is_bounded\`
- **FORM-09**：\`form_09_multi_form_selection\`
- **A6 五码互异（补充）**：\`extract_outcomes_have_distinct_codes\`
- **EXTRACT-17 / EXTRACT-08 预算兜底（PR #71 修复）**：\`extract_17_zero_hit_large_page_over_budget_times_out\`、\`extract_17_zero_hit_parse_over_budget_times_out\`、\`extract_17_zero_hit_within_budget_stays_ok_empty\`、\`extract_17_search_zero_hit_over_budget_times_out\`、\`extract_17_deep_descendant_scan_is_bounded\`、\`extract_08_search_advance_matches_regex_iter\`

### 6.8 PR 09 降级语义（议题 #38）

本 PR 叠在 PR 06（#35）之上：新增 `crates/webctl-web/src/degrade/**`（判定 + 诊断 + 信封守卫层），
只在 `extract::select_matches` 与 `transport::fetch` 各加一个接入点，不改 PR 06 的既有语义。

- `crates/webctl-web/src/degrade/detect.rs` 单元 **5 条**：`static_text_short_circuits_even_with_scripts`、`empty_root_with_scripts_is_partial`、`blank_shell_without_placeholder_is_total`、`script_dominated_page_with_chrome_text_is_partial`、`image_only_static_page_is_not_shell`；服务 DEGRADE-01/04/05/06/09 的判定侧。
- `crates/webctl-web/tests/degrade.rs` 集成 **11 条**（离线；本地 fixture + 127.0.0.1）：
  - DEGRADE-01 的 L0 等价物：`degrade_01_l0_js_data_shell_is_degraded`
  - DEGRADE-02 的 L0 等价物：`degrade_02_l0_static_anchor_yields_ten_quotes`
  - DEGRADE-03：`degrade_03_diagnosis_names_all_four_signal_kinds`
  - DEGRADE-04：`degrade_04_noscript_only_page_is_degraded`
  - DEGRADE-05：`degrade_05_empty_main_container_with_scripts_is_degraded`
  - DEGRADE-06（不误判 + EXTRACT-06 边界）：`degrade_06_static_page_with_many_scripts_is_not_degraded`
  - DEGRADE-07：`degrade_07_force_switch_returns_raw_html_with_degraded_from`
  - DEGRADE-08：`degrade_08_zero_hits_on_js_page_is_never_ok`
  - DEGRADE-09：`degrade_09_spa_shell_zero_hits_is_unsupported_with_explanation`
  - DEGRADE-10：`degrade_10_guard_never_rewrites_or_fabricates_content`
  - 取回层接入点（离线回环 fixture）：`degrade_fetch_entry_marks_js_shell_degraded_offline`
- SESS-10 由既有 `crates/webctl-web/tests/session.rs::sess_10_spa_degrades`（连同 `sess_15_paginate_spa_stays_degraded`）覆盖；接入守卫后仍全绿。
- **L1（可跳过、不作 CI 门禁）**：`crates/webctl-web/tests/e2e/run-real.sh` 承载 DEGRADE-01/02 与 E2E-01/02——静态锚点计数 10 条、JS 页断言降级码非 OK 且 `degraded_from` 非空；`WEBCTL_REAL!=1` 或无网络/缺 curl/cargo 时显式 SKIP。

### 6.8a2 #124 降级可行动信息全量覆盖（`webctl_core::actionable` + 扫描用例）

- `crates/webctl-core/src/actionable.rs` 单测 **5 条**：`plus_separated_from_matches_any_token`（加号组合按段匹配，钉住引擎的 from+limit）、`combined_from_matches_any_token`（组合串 `char-budget,max-results` 按段匹配）、`prefix_rules_cover_dynamic_sources`（`content-type:pdf` 前缀规则）、`attach_fills_actionable_but_respects_existing_and_no_channel`（已降级才补、已有不覆盖、`NO_CHANNEL` 不补）、`registry_is_well_formed`（键唯一、reason 非空、通道名是小写工具名）。
- `crates/webctl-mcp/tests/degrade_reasons.rs` **6 条**：`every_degraded_from_in_source_is_registered`（**扫描 `crates/*/src` 里 `Envelope::degraded(` 附近的来源字面量**，未登记即失败）、`fetch_truncation_carries_actionable`（传输截断 → `response-truncated` + `download`）、`extract_max_results_carries_actionable`（`result-limit-reached` + `paginate`）、`table_span_clamp_carries_actionable`（`table-span-clamped`，通道为空数组）、`ok_envelope_has_no_actionable`（干净成功不得带）、`pagination_fetch_failure_carries_actionable`（分页聚合失败 `pagination-fetch-failed` 也带 actionable——复审指出的真实路径）。另：分页的 `pagination-loop` / `pagination-max-pages` / `pagination-degraded-page` 均已在 `webctl_core::actionable` 登记，组合来源按 `,` 与 `+` 两种拼法分段匹配。
- MCP 侧 `limit_degradation_carries_actionable`（原 `limit_degradation_has_no_actionable_yet` 已改写）断言 LIMIT 也带 actionable 且通道是真工具。

### 6.8b #114 内嵌结构化数据（EMBED-01…EMBED-13）

`crates/webctl-web/tests/embedded.rs` **13 条**（离线；只读内存里的 HTML 字符串，不联网、不执行 JS）：

- **EMBED-01**：单块 JSON-LD 解析成可寻址 JSON；
- **EMBED-02**：同页多块 JSON-LD（含数组形式）保序返回；
- **EMBED-03**：`<script type="application/json" id="__NEXT_DATA__">`；
- **EMBED-04**：`window.__INITIAL_STATE__={…}` 内联状态（**且断言不因变量表重叠而重复**）；
- **EMBED-05**：`window.__NUXT__=(function(){…})()` 是 IIFE 不是 JSON —— **不许硬猜**；
- **EMBED-06**：容器在但 JSON 坏了 → 信封必须是 `PARSE`；
- **EMBED-07**：Inertia `data-page="…"`（HTML 实体转义）；
- **EMBED-08**：页面没有内嵌数据 → **合法零结果**（`ok=true` + `OK`），不是 `PARSE`；
- **EMBED-09**：超字节上限 → `truncated=true` 且不解析、不假装成功；
- **EMBED-10**：UTF-8 多字节内容不得让扫描器 panic；
- **EMBED-11**：`type="application/ld+json;charset=utf-8"` 仍要认出；
- **EMBED-12**：条数上限生效（防畸形页面撑爆结果）；
- **EMBED-13**：`window.X` 与裸 `X` **各自出现**时必须是两条（防止去重修过头）。

**工具面守卫**：`scripts/check-tool-face.py`（CI 的 `guards` 作业）机械校验"`TOOL_NAMES` = PLAN §3.7 = SKILL.md = 现行文案里的数字"；三种变异（文档数字 / README 清单 / SKILL 清单）都能让它红。

模块内另有 **4 条**单测（`parse::embedded::tests`）：字面量扫描处理字符串/转义、拒绝 IIFE、深度上限、实体反转义。

**反向对照**：把 `script_blocks` 改成不产出任何载荷后，13 条里 **9 条 FAILED**（证明用例不空转）。

### 6.8c #119 SPA 样本集与基线（`tests/spa_baseline.rs`）

`crates/webctl-web/tests/spa_baseline.rs` **2 条**：5 类 SPA 形态的**可执行基线**（正文字符数、内嵌条数、
是否判为依赖 JS、RSS 线索、`obtained` 分级）+ 分级函数本身的区分性用例。

样本在 `crates/webctl-web/tests/fixtures/spa/`（nextjs / nuxt / 纯 CSR 空壳 / 静态文章+JSON-LD / 静态博客+RSS）。
数字与口径见 [docs/SPA-BASELINE.md](SPA-BASELINE.md)；**改行为必须同时更新该文件与本节基线**。

### 6.8d #127 沙箱核心 `crates/webctl-js`（ADR-0005 第 1 片）

`crates/webctl-js` **20 条**（离线，纯内存；**不联网、不执行外部进程**）：

**沙箱核心（10 条）**：

- 同一上下文共享全局：两段脚本依次求值，`acc*21` → `42`；
- **限值可中断**：`while(true){}` → `Limit`（实测默认限值下 **609 ms** 被中断）；深递归 → `Limit`（实测 **3.4 ms**）；
- **输入预算**：单段超尺寸、脚本段数超限 → `Rejected`（**不进入引擎**）；
- **零宿主能力**：`typeof` 逐项断言 `fetch` / `XMLHttpRequest` / `require` / `process` / `Deno` / `WebSocket` / `setTimeout` / `setInterval` / `Buffer` **全部 `undefined`**；
- **动态 `import()` fail-closed**（安全回归，见下，**两条**）：绝对路径指向的**存在的**模块文件不会被加载（断言拿到 "module resolution is disabled" 且宿主全局未被设置）；裸**相对说明符**同样 fail-closed（最危险的 CWD 形态）；
- **确定性（有边界）**：不注入**宿主**时间/随机源；ECMAScript 自带的 `Date`/`Math.random` 仍在，依赖它们的脚本可能不同；
- 语法错误归类为 `Syntax`。

> 诚实边界：`boa_engine` 无抢占式中断，故**不承诺**"任意脚本 X 毫秒内结束"；死循环/深递归靠 loop/recursion 限值中断，直线型长脚本只受字节上限约束。
>
> **安全教训（独立审阅实证的真实逃逸）**：boa 0.20 的 `Context::default()` 会装上以**进程 CWD 为根**的
> `SimpleModuleLoader`，于是 `import('相对路径.js')` 会真的 open/read 宿主文件（审阅用 FIFO 阻塞 2.000 s 证明）。
> 修复是显式装 `IdleModuleLoader`（fail-closed）。**结论：用第三方引擎时，"默认上下文"本身就是攻击面，必须逐项确认它默认装了什么。**

**最小 DOM 渲染（10 条，议题 #128 第 1 半）**：

- `renders_document_write_page`：仓库自己的 JS 空壳样本（`js_data_script_shell.html`）渲染后，
  DOM 里出现 **10 条** `class="quote"`（原本只在 `document.write` 的脚本里）；
- `renders_against_existing_dom`：`document.querySelector('#app').textContent = …` 能改到静态 DOM；
- `skips_external_and_module_scripts`：`<script src>` 与 `type="module"` **不执行**（前者引入网络依赖，后者需模块加载器且已 fail-closed）；
- `dom_shim_adds_no_host_capabilities`：**装好 DOM 之后能力面不变**——脚本自己把 `typeof fetch/require/process/setTimeout/XMLHttpRequest`
  写进 DOM，断言全是 `undefined`；
- `render_respects_script_budget`：脚本段数超限直接拒绝；
- `render_is_deterministic`：同一页面两次渲染结果一致；
- `render_limits_apply_to_page_scripts`：**页面脚本也受限值约束**（脚本里 `while(true){}` → `Limit`，不会挂住渲染）；
- `page_cannot_overwrite_serializer`：页面**不能**改写/删除序列化函数（`writable: false, configurable: false, enumerable: false`），否则可伪造渲染结果；
- `renders_document_write_page` 有两条互补断言，作用不同（**§复审实测校正**）：
  真正**判别 RAWTEXT 回归**的是 `contains("class='quote'")`（脚本文本按原文保留）；
  而"完整输出里 `class="quote"` 正好 10 次"**单独并不能判别**——若脚本正文被当 DOM 解析，
  序列化对 script 子节点会输出 `[object Object]`，幽灵节点不进输出，计数仍是 10（已用对照实验证实）；
  该计数断言用于防"幽灵节点泄漏到输出"这一类回归。
- `script_inside_rawtext_is_not_executed`：`<title>`/`<style>`/`<textarea>` 里的 `<script>` **不是脚本**（浏览器不执行），Rust 提取器同样跳过；
- `rawtext_serializes_verbatim`：RAWTEXT 元素内容**按原文**序列化（不转义），与浏览器 `outerHTML` 一致。

> 垫片是**纯 JS 字符串**（`src/dom_shim.js`），不注册任何 Rust 宿主函数；实现子集：解析、`document.write`、
> `createElement`/`appendChild`、属性、`textContent`、`innerHTML`（读+写）、`querySelector(All)`（`#id`/`.class`/`tag`）、序列化。
> **不提供**：定时器（`setTimeout` 永不触发是"静默省略"，不如直接不提供）、网络/存储、真实事件语义（`addEventListener` 空实现）、布局。
> 渲染结果**忠实保留**原始 `<script>` 元素（与浏览器一致），**且脚本正文按原文保留**（RAWTEXT：`script`/`style`/`textarea`/`title` 的内容不再被当 DOM 解析）。
> 因此统计"页面内容"时要先剥掉脚本正文——本节的用例都遵守这条。

依赖边界守卫新增归属校验：`boa_engine` **只允许**被 `webctl-js` 依赖（`scripts/check-dependency-boundary.sh`）。

> **已知边界（不许含糊）**：boa 0.20 无指令数/墙钟/堆内存上限 API，故本片**不限制堆分配**
> （实测 `new Uint8Array(256 MB)` 成功、约 0.9 s）；`safe` 的 CPU 防护只有 loop/recursion/stack 限值 + 输入预算，
> 内存需配合进程级 `ulimit`。归属校验用 `cargo metadata` 依赖图（不 grep 清单，避免缩进/子表/重命名绕过）。

### 6.8e #117 站点级通道与元数据（`parse/site_channels.rs`，第 1 步）

- `crates/webctl-web/src/parse/feed.rs` 单元 **5 条**（#117 第 2 步）：`parses_rss_items_in_order`、`parses_atom_links_via_href`（Atom 的 `link href`）、`unknown_document_is_empty_with_note`、`empty_feed_is_not_an_error`、`entity_decoded_once`。
- `crates/webctl-web/src/channels.rs` 单测 **2 条**：状态序列化、`oembed_is_skipped_without_network`（oEmbed 只报声明、不拉取）。
- `crates/webctl-mcp/tests/channel_fetch.rs` **5 条**（#117 第 2 步接线，纯本地 fixture）：`feed_channels_are_off_by_default`（默认 **零请求**）、`feed_channels_opt_in_parses_rss`（取回并解析条目；**og:title 只作为 og-meta 元数据条目**，不冒充正文）、`feed_channels_opt_in_parses_atom`、`robots_disallow_blocks_feed_without_request`（**requests==0** 且 status=blocked）、`all_channels_failing_degrades_explicitly`（全失败 → `DEGRADED` + `channel-fetch-failed` + `feed-unavailable`）。
- `crates/webctl-web/src/parse/site_channels.rs` 单元 **11 条**（议题 [#117](https://github.com/Thatgfsj/webctl/issues/117) 第 1 步，**尚未接线**）：
  `metadata_carries_an_explicit_source`（**每条元数据带 `source`**，据此区分"元数据"与"正文"——验收要求）、
  `channels_are_discovered_in_document_order`（rss/atom/oembed 保文档序）、
  `empty_page_yields_empty_not_error`（找不到就是**空**，不是错误）、
  `non_alternate_and_unknown_types_are_ignored`（`rel=stylesheet` 带 RSS MIME **不算**通道）、
  `tag_boundary_prevents_false_matches`（`<metadata>` 不匹配 `<meta`）、
  `attribute_values_tolerate_quoting_styles`（单/双引号与**无引号**值；曾把 `https://…` 截成 `https:` 的 bug 由它抓出）、
  `fake_meta_in_comments_and_scripts_is_ignored` / `fake_title_in_comment_is_ignored`（注释、脚本、样式里的假标签**不得**被采信）、
  `angle_bracket_inside_quotes_does_not_split_tag`（引号内的 `>` 不终止标签）、
  `entities_are_decoded_exactly_once`（`&amp;lt;` 只解一次）、
  `ignored_ranges_does_not_swallow_real_metadata`（[#148](https://github.com/Thatgfsj/webctl/pull/148) 复审的 **6 个误伤反例**：属性值/注释/样式/脚本里的 `<script>` 与未闭合引号，都**不得**吞掉真实元数据）。
  > `PR #149`（未合并）另加 3 条洪泛/预算用例；合并后本节应为 **14 条**。


### 6.8f web_search（0.2.2 M1，议题 [#200](https://github.com/Thatgfsj/webctl/issues/200)）

- `crates/webctl-web/src/web_search/` 单元 **14 条**：引擎名解析与大小写、抖动区间/可复现/覆盖全域/盐哈希、Bing SERP 解析、`/ck/a` 解包（合法/无前缀/非法 base64/非 http(s)）、host 归属判断（2 条结果 + 相关主题、max_results 截断、空页、验证码特征、URL 编码与分页、结果 URL 规范化、逐引擎状态映射）；
- `crates/webctl-mcp/tests/web_search.rs` **6 条**（纯本地 fixture，零外网）：结果解析 + 合规/不可信披露 + **只发 1 次 SERP 请求** + 抖动下界、分页 first、拦截页 → `DEGRADED + search-blocked + actionable`、零结果 → `DEGRADED + search-empty`、未接线/未知引擎/空 query 不联网、`max_results` 截断（上限 30）。
- 合规开关：`WEBCTL_SEARCH_STRICT_ROBOTS=1` 交给 polite 层拦截（单测只覆盖 note 文案；联网行为由 M4 真站慢速验证记录）。

### 6.9 PR 11 MCP server（议题 #19）

`crates/webctl-mcp/src/` 单元 **29 条**（0.2 M1 增 `site.rs` 的 6 条 `resolve_site` 用例；0.2 M3 再增 3 条 `classify` 口径用例；0.2 M5 再增 4 条：`classify_serp_failure_is_named_and_keeps_evidence`、`classify_reports_both_l3_and_serp_failures`、`serp_fallback_is_off_by_default`、`serp_fallback_never_networks_in_offline_mode`）：`registry_is_exactly_the_plan_list`、`definitions_match_the_static_specs`、`qualified_names_follow_the_dsh_contract`、`all_plan_tools_are_registered_and_available`、`capabilities_mirror_the_dispatch_table`、`instructions_stay_under_limit`、`dsh_snippet_has_no_tool_order_and_no_tool_names`、`bound_session_accepts_only_itself`、`cross_session_error_is_readable_and_mentions_both`、`unbound_defaults_to_default_and_validates`、`small_result_stays_inline`、`large_result_is_spilled_and_keeps_degradation`、`sec_09_mcp_dispatcher_is_the_one_that_redacts`（[#82](https://github.com/Thatgfsj/webctl/issues/82) 补的隔离用例，详见本节末尾）；0.2 M1 的 `site.rs` 另有 **13 条**（0.2 M5 增 4 条，见下）：`builtin_hit_is_confident_and_not_degraded`、`direct_only_in_offline_mode_degrades_and_says_so`、`unknown_name_returns_empty_and_degraded`、`url_argument_is_bad_url`、`empty_site_is_parse_error`、`max_candidates_is_clamped`（以上 6 条为工具面），以及 0.2 M3 新增的 `classify_l3_failure_wins_over_other_reasons`、`classify_offline_says_offline`、`classify_no_match_vs_low_confidence`（降级原因口径，不依赖网络）。

`crates/webctl-mcp/tests/mcp.rs` **35 条通过 + 0 ignored**（#82 转正后 20 条，#114 增 3 条 + 2 条 schema 守卫，#115 增 5 条，#119 补 1 条，#128 接线增 4 条）；另 `tests/resolve_site.rs` **3 条**（0.2 M1，单独成文件以免 `mcp.rs` 破 1000 行）：

- **上游降级继承**（#125，`tests/degradation_inherit.rs`，**4 条**，纯本地 fixture）：`doc_text_inherits_upstream_js_render_degradation`（JS 空壳页 → `DEGRADED` + `degraded_from=js-render` + actionable，文本仍返回）、`doc_text_on_static_page_stays_ok`（反向守卫：静态页仍 `OK` 且无 `degraded_from`）、`download_inherits_upstream_js_render_degradation`（同样继承且文件确实落盘）、`json_query_keeps_parse_failure_instead_of_inheriting`（**只继承 OK**：本工具自己的解析失败不得被伪装成降级）。三个工具都以 URL 为输入、自己重建信封；此前会丢掉上游 meta 而静默返回 OK。

- **resolve_site**（0.2 M1，`tests/resolve_site.rs`；L3 落地后低置信用例改为 `offline` 上下文，避免测试联网）：`resolve_site_builtin_hit_is_wired`（内置命中 = `OK` + `confidence=high` + **无** actionable）、`resolve_site_low_confidence_degrades_with_real_next_steps`（只有直推 → `DEGRADED` + `degraded_from=resolve-site` + `next_steps` 用派发表反查）、`resolve_site_schema_declares_only_wired_parameters`（**声明必须等于实现**：0.2 M1 只有 `site` / `max_candidates`，0.2 M5 增 `allow_serp_fallback` 后恰为三个参数）


- **L2 通道**（0.2 M2，`tests/resolve_site_site_search.rs`，**5 条**，纯本地 fixture）：`matching_external_links_become_candidates`（只收名称匹配的外部链接，内部/无关链接被过滤，单站点 **55 分**）、`two_sites_hitting_raises_corroborated_score`（≥2 个站点都命中 → **58 分**并标『互证』，**仍低于可靠阈值 60**）、`robots_disallow_is_reported_per_site`（robots 禁止时**不请求**且 note 保留 robots 原因）、`page_without_matches_is_empty`（可读但无匹配 = Empty）、`max_sites_limits_probing`（探测站点数受上限约束）。另有 `resolve_site::site_search` 单测 **7 条**（模板数量 20–50、https + 占位符、名称匹配、后缀规范化、查询编码、短名不误配、host label 命中）；两种分值低于可靠阈值由**编译期断言**钉住（不是运行期用例）。

- **L3 通道**（0.2 M3，`tests/resolve_site_l3.rs`，6 条，纯本地 fixture）：`p856_becomes_official_site_candidate`（搜索 → P856 → public-api 候选，http 规范成 https、去尾斜杠，证据含 Q 号与标签）、`robots_disallow_stops_before_requesting`（robots 禁止时**不发出**搜索请求且原因是 robots）、`empty_search_is_no_match`、`entity_without_p856_is_no_official_site`、`non_json_search_is_parse_failure`、`entities_error_is_reported_with_code`（500 → BLOCKED + 码）。

- **L4 通道**（0.2 M4，`tests/resolve_site_sitemap.rs`，4 条，纯本地 fixture）：`declared_sitemaps_are_reported_in_file_order`（声明按 robots.txt 顺序报告、只抓一次）、`robots_without_sitemap_is_none_not_unavailable`（可读但无声明 = None）、`missing_robots_txt_is_unavailable`（404 = Unavailable，不得说成『没声明』）、`probe_top_respects_limit_and_keeps_candidate_order`（探测上限与候选顺序）。另有 `politeness/robots.rs` 单测增 2 条（`sitemap_lines_are_collected_case_insensitively`、`no_sitemap_lines_means_empty_declaration`）。

- **L5 通道**（0.2 M5，`tests/resolve_site_serp.rs`，4 条，纯本地 fixture）：`serp_results_become_low_score_candidates`（结果 → 低分候选、证据带名次、引擎链接被过滤、跳转链接还原）、`max_results_is_respected`、`robots_disallow_stops_before_requesting`（robots 禁止时**不请求**）、`empty_page_is_empty_failure`。另有 `resolve_site::serp` 单测 6 条（编码/引擎过滤/跳转还原/协议过滤/原因码）。

- **MCP-01**：`mcp_01_initialize_negotiates_protocol_version`
- **embedded_data**（#114）：`embedded_data_extracts_json_ld_from_inline_html`、`embedded_data_empty_result_is_not_a_parse_failure`、`embedded_data_broken_payload_is_parse_error`、`embedded_data_schema_declares_only_wired_parameters`（**声明必须等于实现**）、`embedded_data_rejects_url_and_html_together`
- **引擎接线**（#128 步骤 2）：`js_render_off_keeps_existing_degradation`（**默认关闭**：脚本页仍 DEGRADED、0 命中）、
  `js_render_on_extracts_content_from_script_only_page`（**开启后**：内容只在 `document.write` 的页面抽出 **10 条**）、
  `js_render_failure_is_degraded_not_silent_success`（渲染失败 → `DEGRADED` + `degraded_from=js-render-failed` + `actionable`，**不是**成功空结果）、
  `caps_reports_js_wired_with_coverage`（`caps.js.wired=true` 且 `reason` 说清触发条件与能力边界）；

- **可行动降级**（#115）：`spa_surface_channels_are_registered_tools`（建议通道必须是真实工具）、`degraded_envelope_carries_actionable_next_steps`（降级信封带 reason/detected/next_steps，且 next_steps 用派发表反查）、`ok_envelope_has_no_actionable`（干净成功不得带 actionable）、`limit_degradation_has_no_actionable_yet`（**口径边界**：LIMIT 降级不带 actionable；带 actionable 的降级目前是 JS 渲染与 `resolve_site` 低置信）、`paginate_propagates_actionable_from_upstream`（**传播不丢信息**：复审实测 paginate 曾丢 actionable，此用例钉住修复）、`paginate_normal_finish_keeps_actionable`（#119 补：#123 复审发现原用例只覆盖 `pagination-fetch-failed` 早返回，**正常收尾**分支此前没被钉住）
- **MCP-02**：`mcp_02_tools_list_is_exactly_the_plan_set`
- **MCP-03**：`mcp_03_input_schemas_are_valid_json_schema`
- **MCP-04**：`mcp_04_qualified_names_follow_the_function_name_contract`
- **MCP-05**：`mcp_05_instructions_stay_under_32k`
- **MCP-06**：`mcp_06_two_servers_start_without_conflict`
- **MCP-07**：`mcp_07_cross_session_calls_are_rejected`
- **MCP-08**：`mcp_08_no_tool_order_in_config_or_instructions`
- **MCP-09**：`mcp_09_call_budget_times_out_as_envelope`
- **MCP-10**：`mcp_10_tool_failures_use_the_envelope`
- **MCP-11**：`mcp_11_large_results_are_spilled_to_disk`
- **MCP-12**：`mcp_12_skill_md_matches_tools_list`（另有 `scripts/check-skill-tools.sh` 与 `webctl mcp --list-tools` 同源校验）
- **MCP-13 / STATE-06**：`mcp_13_shares_state_dir_with_core_resolution`
- **SEC-09**：`sec_09_mcp_content_is_untrusted`
- **ENV-06**：`env_06_cli_and_mcp_map_the_same_error_code`
- 补充：`all_tools_are_dispatchable`
- **#82 收口（DEGRADE-* 的 MCP 侧接线 + SEC-09；原 `mcp_js_degradation_pending_pr09` 转正并更名）**：
  - `mcp_js_degradation_propagates`：JS 页（fixture `js_data_script_shell.html`）在 MCP `fetch` / `extract` / `paginate` 上 `code` 非 OK 且 `meta.degraded_from` 非空、`trust=untrusted_page_content`；静态对照页零命中仍是 `ok=true + code=OK + 空数组`（EXTRACT-06）。
  - `mcp_deg_search_and_listing_paths_follow_pr09`：`search` / `links` / `tables` / `forms` 在 JS 页零结果同样非 OK；静态页 `search` 零命中仍是 OK 空结果。
  - `mcp_deg_code_matches_library_on_same_input`：同一 fixture 输入下 MCP 与库路径（`transport::fetch` / `extract::select_matches` / `extract::search`）的 `code` 逐一相等。
  - `sec_09_mcp_fetch_redacts_credentials`：MCP 页面内容 `trust=untrusted_page_content`，信封不含 `set-cookie` / `Authorization` / URL query token 明文；脱敏入口 `dispatch::redact_fetched_envelope` 复用 `webctl_web::redact::redact_fetched`。

PR 11 留下的 `#[ignore]` 交接哨兵及其常量已随 #82 删除；随后 `crates/webctl-web/tests/offline_lane.rs` 的两条历史占位也清掉（DOC-12 改为**真实调用** `doc_text` + `json_query`；与本仓 `tests/mcp14.rs` 重复的 MCP-14 占位**删除**）。**全仓 `cargo test` 现为 384 passed / 0 failed / 0 ignored —— 不再有任何 `#[ignore]` 属性。**

`crates/webctl-mcp/tests/mcp14.rs` **1 条**：`mcp_14_in_process_dispatch_spawns_no_children`（单独二进制，避免其他用例的子进程污染基线；复用 PR 13 的 `tests/support/resources.rs`）。

议题 #68（caps 诚实性）：`crates/webctl-core/src/caps.rs` 新增 `missing_tools_never_report_interface_available`、`unavailable_tools_do_not_light_up_interface`、`registered_tools_light_up_interface`；`webctl caps --json` 现枚举 §3.7 的 17 个工具（#114 起 16 个，0.2 M1 起 17 个）并逐项给 state/reason，`interface` 只在工具真注册且可用时 available。

服务 ENV-06、STATE-06、MCP-01…14、SEC-09。
- **MCP-SEC09-ISO（隔离度补充）**：`crates/webctl-mcp/src/dispatch.rs` 的 `sec_09_mcp_dispatcher_is_the_one_that_redacts` —— 手工构造**仍含明文凭据**的 `Fetched`，只调 `dispatch::redact_fetched_envelope`（**不经过** `transport::finish`），断言明文消失且非敏感头保留。
  为什么需要它：走完整取回流程的用例（`sec_09_mcp_fetch_redacts_credentials`）**删掉 MCP 侧那次调用仍会通过**（取回层已脱敏），只能证明"系统整体会脱敏"；本用例删掉 `redact_fetched` 调用则**必须失败**（反向对照已实测）。

### 6.9b #183 浏览器上下文：CLI 开关与两处注入

`crates/webctl-web/tests/browser_context_session.rs` **4 条**（原始 socket fixture，记录请求头）：`session_get_injects_context_headers`（会话 GET 注入 UA / accept / accept-language / referer + 同域 cookie，跨域 cookie 不出现）、`session_post_injects_context_headers`（**表单 POST** 同样注入——此前 `SessionEngine::send` 不读上下文）、`jar_cookie_wins_over_context_cookie`（jar 发 Cookie 时上下文让位，只有一个 Cookie 行与一个 UA）、`without_context_nothing_is_injected`（无上下文时不注入，行为不变）。

`crates/webctl/tests/cli.rs` 增 **5 条**：`browser_context_flag_injects_headers_on_fetch`（`--browser-context` 的 UA / accept-language / cookie 真的到了 fixture）、`no_browser_context_overrides_env`（`--no-browser-context` 压过 `WEBCTL_BROWSER_CONTEXT`）、`env_browser_context_is_still_honored`（不给开关仍认环境变量）、`browser_context_flags_are_mutually_exclusive`（clap 退出码 2）、`broken_browser_context_file_fails_loudly`（显式文件坏了必须失败，不静默）。共享装载逻辑在 `webctl-web/src/browser_context.rs` 的 `parse_json` / `load_file` / `load_from_env`，MCP 与环境变量入口复用同一实现。

### 6.10 PR 12 安全与凭据（议题 #21）

默认严格，全部用例在默认配置下跑（L3），零外网、零子进程。

- **`crates/webctl-web/tests/security.rs` 9 条**：`sec_04_envelope_redacts_cookie_and_query_token`（SEC-04）、`sec_06_cross_origin_redirect_drops_authorization`、`sec_06_same_origin_redirect_keeps_authorization`（SEC-06，含对照组）、`sec_07_url_query_tokens_are_redacted_in_logs`（SEC-07）、`sec_08_password_form_fields_are_redacted_in_logs`（SEC-08）、`sec_14_download_whitelist_and_size_limit`、`sec_14_commit_download_enforces_policy_and_containment`（SEC-14）、`sec_11_file_arguments_are_canonicalized`（SEC-11）、`sec_15_dangerous_schemes_are_rejected`（SEC-15）。
- **`crates/webctl/tests/security.rs` 7 条**：`sec_01_sec_02_default_strict_and_export_confirmation`（SEC-01 + SEC-02）、`sec_03_export_file_is_mode_600`（SEC-03）、`sec_05_cookie_values_never_reach_output_or_logs`（SEC-05）、`sec_07_url_query_token_is_redacted_in_logs`（SEC-07）、`sec_11_cli_file_arguments_are_contained`（SEC-11：`../`、绝对路径、越界 symlink、坏会话名）、`sec_13_errors_carry_no_stack_or_paths`（SEC-13）、`sec_15_dangerous_scheme_via_cli`（SEC-15）。
- **新增单元/护栏**：`webctl-core::security::redact`（`cookie_values_are_redacted_but_names_kept`、`authorization_is_fully_redacted`、`url_query_tokens_are_redacted`、`log_line_redacts_urls_headers_and_form_fields`）、`webctl-core::security::paths`（`rejects_traversal_and_absolute_arguments`、`allows_relative_argument_under_root`、`rejects_symlink_escape`、`secret_file_is_mode_600_even_if_previously_wider`、`scheme_whitelist_rejects_file_and_data`）、`webctl-web::redact`（`envelope_redaction_removes_secrets_everywhere`、`session_state_display_never_contains_cookie_values`）、`webctl-web::paths`（`whitelist_and_size_limit_are_enforced`、`commit_download_writes_inside_root_only`、`commit_download_rejects_traversal_and_symlink`）、`webctl::logs`（`cookie_values_never_reach_the_log`、`url_query_tokens_are_redacted_in_logs`、`password_form_fields_are_redacted_in_logs`）。

覆盖与交接（对齐 §2.14）：

| 编号 | 覆盖位置 | 说明 |
|---|---|---|
| SEC-01 | `webctl/tests/security.rs` | 帮助中无放宽开关；默认配置下导出仍需 `--yes` |
| SEC-02 | 同上 | 无 `--yes` → `BLOCKED`，且不创建文件 |
| SEC-03 | 同上 | envelope `mode=384` 与文件系统 `0600` 双向验证 |
| SEC-04 | `webctl-web/tests/security.rs` | transport `finish` 即脱敏；正则断言无明文 |
| SEC-05 | `webctl/tests/security.rs` + `webctl::logs` 单测 | fixture 注入 cookie，输出/日志均无明文 |
| SEC-06 | `webctl-web/tests/security.rs` | 自造跨域 fixture；跨源丢 `Authorization`，同源保留（对照组） |
| SEC-07 | 两侧 + `webctl-core` 单测 | URL query token 正则断言脱敏，且无重复占位 |
| SEC-08 | `webctl-web/tests/security.rs` + `webctl::logs` | 表单 `password=` 字段不落日志 |
| SEC-09 | **交接 PR 11（MCP）** | 见本 PR 描述「未做到项」；MCP 入口未在本 PR 落地 |
| SEC-10 | 复用 PR 13 | 未重复实现 |
| SEC-11 | 两侧 + `webctl-core::security::paths` | `resolve_file_argument` 拒绝绝对路径，`ensure_contained` 拒绝 `..` 与越界 symlink；已系统性覆盖 cookies 导出路径与下载路径 |
| SEC-12 | 复用 PR 05 | 未重复实现 |
| SEC-13 | `webctl/tests/security.rs` | 正则断言无 `panicked` / `.rs:N` / `backtrace` / 本地路径 |
| SEC-14 | `webctl-web/tests/security.rs` | 类型白名单 + 尺寸上限 + containment + 落盘 0600 |
| SEC-15 | 两侧 + `webctl-core::security::paths` | `file://`/`data:`/`ftp://` 在连接前 `BAD_URL` |

**植入实验**（证明断言可失败，非恒真）：临时令 `redact_log_line` 变为恒等 → SEC-07/08 两侧失败并回显 `SUPER_SECRET_TOKEN`；临时跳过 `resolve_file_argument` 的 `ensure_contained` → SEC-11 两侧失败。两次均 `git checkout` 还原并复跑通过；证据见 PR 描述。

### 6.11 L4 性能基线（议题 #83）

单测的墙钟断言已被放宽为 30s「挂死守卫」（PARSE-14 / EXTRACT-14 / EXTRACT-17），
`[~5s, 30s)` 一档因此**没有门禁**；绝对墙钟不是单测该用的工具。本小节把它交给 **L4 通道**：
`crates/webctl-web/examples/perf_probe.rs`（确定性输入，自生成，不落盘 fixture）+
`scripts/perf-baseline.sh`（相对基线 + 宽 tolerance，`--record`/`--check`），基线存 `docs/perf-baseline.json`。

| 编号 | 用例 | 输入 | 期望 |
|---|---|---|---|
| PERF-01 | 解析 100KiB 嵌套 HTML（`parse::select`） | 自生成 ~100KiB | p50 ≤ max(基线 × 5, floor_ms) |
| PERF-02 | 抽取 1MiB **多命中**（`select_matches`，放开输出上限） | 自生成 ~1MiB | p50 ≤ max(基线 × 5, floor_ms) |
| PERF-03 | 抽取 1MiB **零命中全量扫描**（PR 06 缺陷路径） | 自生成 ~1MiB | p50 ≤ max(基线 × 5, floor_ms) |
| PERF-04 | 深嵌套（2000 层）抽取 | 自生成 | p50 ≤ max(基线 × 5, floor_ms) |

**有效门槛**（= `max(基线 × 5, floor_ms)`；`floor_ms` 由 `docs/perf-baseline.json` 提供，环境变量可覆盖）：

| 用例 | 基线 p50 | 有效门槛 | 相对基线 | 说明 |
|---|---|---|---|---|
| PERF-01 | 3.20 ms | **100 ms** | ≈31× | 由 `floor_ms` 主导：微小夹具在负载机器上会抖到 10–20ms，绝对下限吸收噪声；只抓灾难级退化 |
| PERF-02 | 2088.05 ms | **10440 ms** | 5× | 真正的主力：`[~5s, 30s)` 档由它抓住 |
| PERF-03 | 63.99 ms | **320 ms** | 5× | 同 5× |
| PERF-04 | 9.52 ms | **100 ms** | ≈10.5× | 同 PERF-01，下限主导 |

**跑法**：`bash scripts/perf-baseline.sh --check`（手动通道，**未接入 CI** —— 每次 PR 跑基准会与被测目标争 CPU，反而制造假红）。发布前必跑（见 [docs/RELEASE.md](RELEASE.md)）。

**门槛语义（诚实声明）**：本通道抓**量级级退化**（如 2.1s → 19.7s），**不**抓 20% 级别抖动
（抖动由 5× tolerance 吸收）；它**不替代**单测的挂死守卫，两者互补。
首次记录（2026-10-17，commit `a6edc00`）：PERF-01 3.20ms、PERF-02 2088.05ms、PERF-03 63.99ms、PERF-04 9.52ms。

**判别力验证**（作者提供，可复现）：把 PERF-02 的抽取人为重复 6 次 → 实测 19,757.96ms > 上限 10,440.27ms，
`--check` 报 FAIL 并退出码 1；还原后全部 OK。
## 7. 维护规则

1. 新增用例必须同时写 **通道** 与 **归属 PR**，否则不合并。
2. 删除用例或放宽期望，必须单独开议题并说明理由（不允许在实现 PR 里顺手改）。
3. 用例编号**不复用**：删除后编号作废，新用例取下一个序号。
4. 每条实现的用例在 PR 描述里勾选并贴命令输出；未落地的一律标 ⬜ 未落地。
5. 本清单与 [PLAN §5.2](PLAN.md) 的 PR 清单同步演进：PLAN 增删 PR 时，本清单的 §3 表必须同步。
6. **每次新增用例，必须同时在 §6「实现回填」登记测试名**（含它服务哪条编号）。审阅者会拿 §3+§6 当验收依据：只写在代码里、不登记到这里的用例，等于没交付。
