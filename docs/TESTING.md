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

v0.4 **没有浏览器**，因此也不再有"无浏览器 lane / mock-cdp lane"之分。

| 通道 | 含义 | 命令 |
|---|---|---|
| **L0** | 离线：fixture HTTP 服务 + 本地文件，**不访问外网** | `cargo test --features offline` |
| **L1** | 真站 smoke（需要网络，可跳过） | `./tests/e2e/run-real.sh` |
| **L2** | 基线：发布物体积 | `cargo build --release --target x86_64-unknown-linux-musl` |
| **L3** | 安全负面：在**默认配置**下跑（不加任何放宽开关） | 并入 L0/L1 |

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
| ENV-07 | schema 快照 | golden 快照比对字段名与类型；新增字段需显式更新快照 | L0 | PR 01 |
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
| PARSE-14 | 解析预算 | 100KB HTML 解析 < 500ms 且计入 `elapsed_ms` | L0 | PR 03 |
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

### 2.7 EXTRACT — 抽取与选择器（16）

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
| CLI-09 | 管道友好 | 单行 JSON 输出可直接管道给下游工具 | L0 | PR 10 |
| CLI-10 | 无凭据输出 | 任何输出/日志不含 cookie/令牌（正则） | L0+L3 | PR 12 |
| CLI-11 | 环境变量文档 | 代码使用的环境变量全部在文档中列出 | L0 | PR 10 |
| CLI-12 | 启动开销 | 冷启动（`--version`）< 50ms，不加载网络栈 | L2 | PR 14 |

### 2.13 MCP — 工具面与 DSH 契约（14）

| ID | 用例 | 期望（可机械判定） | 通道 | 归属 |
|---|---|---|---|---|
| MCP-01 | 握手 | `initialize` 返回协商后的 protocolVersion | L0 | PR 11 |
| MCP-02 | 工具清单 | `tools/list` 恰好 15 个工具且名字与 PLAN §3.7 一致 | L0 | PR 11 |
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
| PKG-02 | musl 静态 | `x86_64-unknown-linux-musl` 产物可运行 | L2 | PR 14 |
| PKG-03 | 三平台 | linux/macos/windows 资产齐全 | L2 | PR 14 |
| PKG-04 | 版本 | `--version` 与 tag 一致 | L2 | PR 14 |
| PKG-05 | 体积基线 | 不超过 P0 定下的基线 | L2 | PR 14 |
| PKG-06 | 干净环境 | 无 Rust/无浏览器的干净环境可直接运行 | L2 | PR 14 |
| PKG-07 | 文档一致 | README 安装命令与实际资产一致（无虚构命令） | L2 | PR 14 |
| PKG-08 | 无动态依赖 | `ldd` 显示静态（musl 产物） | L2 | PR 14 |

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
| **体积基线记录** | P0 定下的 release/musl 体积数字 | PKG-05 |

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

`crates/webctl-web/tests/session.rs` **33 条** = 15 SESS + 17 FORM + 1 fixture 自检：

- **SESS**：`sess_01_login_success`、`sess_02_not_logged_in_is_session_lost`、`sess_03_cookie_persists_across_engines`、`sess_04_csrf_is_extracted_and_refilled`、`sess_05_csrf_rotation_is_refetched`、`sess_06_hidden_fields_are_preserved`、`sess_07_form_10_multipart_upload`、`sess_08_pagination_sums_items`、`sess_09_loop_pagination_stops_with_limit`、`sess_10_spa_degrades`、`sess_11_export_import_session_state`、`sess_12_invalid_session_name_is_rejected`、`sess_13_redirect_login_keeps_session`、`sess_14_concurrent_requests_share_session`、**`sess_15_paginate_spa_stays_degraded`**（首轮审阅否决后补：分页必须传播 DEGRADED，不得吞成 OK+0 条）。
- **FORM**：`form_01_single_field`、`form_02_multiple_fields_at_once`、`form_03_unicode_value`、`form_04_select_value`、`form_05_checkbox_and_radio_semantics`、`form_06_hidden_field_default`、`form_07_unknown_field_is_error`、`form_08_missing_required`、`form_11_landing_url_and_status`、`form_12_duplicate_submission_refused`，**新增** `form_13_duplicate_checkbox_first_checked_only`、`form_14_duplicate_checkbox_second_checked_only`、`form_15_duplicate_text_inputs_keep_order`、`form_16_select_multiple_submits_all_selected`、`form_17_307_preserves_post_body`、`form_18_308_preserves_post_body`、`form_19_303_switches_to_get_without_body`（首轮审阅否决后补：同名多值与 307/308 丢 body）。
- 自检：`fixture_multipart_parser_sanity`。

### 6.4 PR 07 文档与数据（议题 #36）

`crates/webctl-web/tests/doc.rs` **13 条通过 + 1 条 ignored**：`doc_01_text_pdf_extracts_keywords`、`doc_02_chinese_pdf_is_not_mojibake`、`doc_03_scanned_pdf_is_unsupported`、`doc_04_encrypted_pdf_is_explicit_error`、`doc_05_corrupt_pdf_is_parse_error`、`doc_06_json_path_selection`、`doc_07_json_filter_and_projection`、`doc_08_invalid_expression_errors`、`doc_09_deferred_boundary_is_explicit`、`doc_10_oversized_input_is_limit`、`doc_11_unknown_format_is_unsupported_with_diagnostic`、`doc_pdf_extract_panic_is_isolated_as_parse`（补：pdf-extract panic 必须隔离为 PARSE）、`four_outcomes_have_distinct_codes`。
**ignored**：`doc_09_acceptance_after_pr03` —— DOC-09（HTML→文本）依赖 PR 03，故意保持失败状态作为"未达成"的机器证据；PR 03 合并后去 ignore 即成为验收（交接见议题 #59）。

### 6.5 PR 08 礼貌与限速（议题 #37）

- `crates/webctl-web/tests/politeness.rs` **8 条**：`robots_disallow_blocks_without_requesting`、`robots_can_be_disabled_and_is_recorded`、`min_interval_is_enforced`、`concurrency_is_capped`、`retry_after_is_respected`、`interception_is_blocked_without_retry_storm`、`server_error_is_blocked`、`network_errors_retry_within_budget`。
- `crates/webctl-web/src/politeness/**` 单元 **17 条**，含首轮审阅否决后补的三条回归：`queued_requests_still_keep_min_interval`（排队场景最小间隔）、`consecutive_user_agent_lines_share_one_group`、`longest_matching_agent_token_wins`。

### 6.6 待回填（PR 已开未合并）

| PR | 已交付用例（实现者报告，合并后回填测试名） |
|---|---|
| 03 解析（#54） | `tests/parse.rs` 23 条，含 XML 深度预检对抗用例（本轮修 XML 栈溢出后新增） |
| 10 CLI（#60） | `cargo test` 133 passed，含 cli 集成 13 条（CLI-01…11）；CLI-12 属 PR 14 |
| 13 测试基建（#55） | golden 2 / http_fixture 9 / offline_lane 4+2 ignored / resource 10；含三条"扫描器不空转"永久护栏 |

## 7. 维护规则

1. 新增用例必须同时写 **通道** 与 **归属 PR**，否则不合并。
2. 删除用例或放宽期望，必须单独开议题并说明理由（不允许在实现 PR 里顺手改）。
3. 用例编号**不复用**：删除后编号作废，新用例取下一个序号。
4. 每条实现的用例在 PR 描述里勾选并贴命令输出；未落地的一律标 ⬜ 未落地。
5. 本清单与 [PLAN §5.2](PLAN.md) 的 PR 清单同步演进：PLAN 增删 PR 时，本清单的 §3 表必须同步。
6. **每次新增用例，必须同时在 §6「实现回填」登记测试名**（含它服务哪条编号）。审阅者会拿 §3+§6 当验收依据：只写在代码里、不登记到这里的用例，等于没交付。
