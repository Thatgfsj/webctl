# ADR-0008：全网搜索通道（`web_search`）与合规边界

- 状态：已采纳（2026-11-10；0.2.2 M1–M3 已落地，M2 摘要暂缓）
- 关联：[ROADMAP-0.2.2](../ROADMAP-0.2.2.md)、[ADR-0004](0004-politeness-by-default.md)（礼貌层）、[ADR-0006](0006-resolve-site-and-browser-context.md)（浏览器上下文）、[#199](https://github.com/Thatgfsj/webctl/issues/199)（EPIC）、[#200](https://github.com/Thatgfsj/webctl/issues/200) / [#201](https://github.com/Thatgfsj/webctl/issues/201) / [#202](https://github.com/Thatgfsj/webctl/issues/202) / [#203](https://github.com/Thatgfsj/webctl/issues/203)

## 背景

用户要求 webctl 从「单站取回/抽取」扩展到「**按查询词全网搜索**」。用户明确给出的取舍（2026-11-01）：

1. **Bing 为主**，默认走 HTML SERP；**搜索优先、合规其次**；
2. 每次搜索请求前加 **50–500ms 随机抖动**；
3. 新增第 **18** 个工具（定名 `web_search`；现有页面内搜索 `search` 保持不变），工具内提供可选引擎让模型自选；
4. **允许简单反爬**（浏览器化请求头 / 会话 cookie 复用 / 429 退避），**不做**验证码、Cloudflare 级对抗、代理/IP 轮换；
5. 可读 Bing 的 LLM 摘要，但**必须回原始链接核对**；
6. 摘要可能只在 **AI 缓存命中**（查询足够热门）后出现 → 2026-11-01 追加决定：**本阶段不做摘要**。

开发期实测事实（2026-11-01，低频探测）：

| 引擎 | 本机可达 | HTML 可解析 | robots 对搜索路径 |
|---|---|---|---|
| Bing（cn.bing.com） | ✅ | ✅ `li.b_algo` | `Disallow: /search` |
| Baidu | ✅ | ✅ `div.result` + `mu` 即真实 URL | `User-agent: *` → `Disallow: /` |
| 360（so.com） | ✅ | ✅ `li.res-list` + `data-mdurl` 即真实 URL | `Disallow: /s` |
| Mojeek | ✅（验证码页） | ❌ | `Disallow: /search` |
| Marginalia | ✅（新版 UI 是 JS 外壳） | ❌ | `Disallow: /search` |
| Yandex | ✅ | 未接线 | `Disallow: /search` |
| DuckDuckGo（html/lite） | ❌ 本网络超时 | — | lite 为 `Allow: /`（唯一合规，但不可达） |

结论：**本网络下不存在「可达 + robots 允许」的免 key 全网搜索通道**。默认走 Bing 是用户的显式取舍，不是“我们认为这样合规”。

## 决策

1. **工具形态**：新增第 18 个工具 `web_search`；现有页面内 `search` 不动。默认 `engines = ["bing"]`。
2. **通道**：HTML SERP（不是官方 API、不引入 key、不引入外部后端）。已接线 **bing / baidu / 360**；**mojeek / marginalia / yandex 明确未接线**——逐引擎如实报 `unavailable` + 原因，且**不发起请求**（不用未验证的选择器硬猜）。
3. **请求姿态（简单反爬，非对抗级）**：浏览器化 UA/Accept/Accept-Language/Sec-Fetch-*、`WEBCTL_BROWSER_CONTEXT` 的会话 cookie 复用、429 退避；**每次请求前 50–500ms 随机抖动**；不处理验证码、不做 Cloudflare 绕过、不做代理/IP 轮换。
4. **合规口径（诚实优先）**：默认**不因 robots 拒绝而放弃**（用户决策：搜索优先），但每份结果都用 `notes` 明示 robots 事实，文档不宣称“合规”；提供 `WEBCTL_SEARCH_STRICT_ROBOTS=1` 严格模式（robots 禁则 **0 请求 + BLOCKED**）。
5. **结果聚合**：URL 归一化（去 `utm_*`/`fbclid` 等跟踪参数、fragment、尾斜杠；**保留非默认端口**）→ 跨引擎去重 → **RRF（k=60）** 融合 → `max_results` 截断；同一 URL 保留多引擎证据（`engines` 数组），`engine` 标签取最好名次的引擎；**不宣称相关性分数**。
6. **失败语义**：单引擎降级（blocked/empty/timeout/http-error/network-error）保留该引擎原因；多引擎**部分失败** → `DEGRADED + search-partial`（结果照常返回）；**全部失败** → `search-all-failed`；**单引擎硬失败**（如连接被拒）按 M1 语义原样 `fail(code, error)`，不伪装成降级。
7. **分页**：`page` 上限 **5**；Bing 映射 `first`，Baidu/360 映射 `pn`；各引擎能力差异如实写在 `notes`。
8. **LLM 摘要：本阶段不做**。`llm_summary` 恒为 `null`；文档/instructions/SKILL 明确写「摘要解析在 0.2.2 M2（本版不返回）」，不得暗示可用。原因：摘要依赖 Bing 的 AI 缓存命中，且浏览器端渲染，纯 HTML 取回不稳定复现 → 不可测。
9. **真站数据不入库**：验证只记录方法、请求数、延迟、结果条数等结论；不提交原始 HTML、不提交结果列表。

## 备选方案（以及为什么没选）

- **官方搜索 API（Brave / Google CSE / Yandex XML / Mojeek）**：合规性最好，但需要 key 与外部服务；与用户此前「不引入外部服务」的偏好冲突，且 0.2.2 目标是无配置可用。
- **自建 SearXNG 实例**：免 key、可控，但多一个需要维护的运行进程；0.2.2 不引入。
- **默认关闭、必须显式 opt-in 才抓 SERP**（`resolve_site` L5 的形态）：合规姿态最保守，但用户明确要求「搜索优先、合规其次」且默认可用。
- **只做站点内搜索聚合**（复用 L2 模板）：不满足“全网搜索”的语义。

## 后果

- **合规风险由用户显式接受**（ADR-0004 的“不承诺绕过反爬”仍然成立：这里做的是“更像普通浏览器”，不是绕过）；文档必须持续如实披露，不得美化。
- **选择器易变**：Bing/Baidu/360 的 DOM 变动会导致 `search-empty`；解析器失败即降级，不返回空成功；合成 fixture 固定回归。
- **结果质量未评估**：0.2.2 只验证“能取回、能解析、失败语义正确”，不评估相关性；真站抽查出现明显噪声（见下），文档如实记录。
- **摘要不可用**：`llm_summary` 恒 `null`；若未来要恢复，前提是能稳定复现“何时有摘要”，否则不可测。
- **严格模式是唯一合规缓解**：`WEBCTL_SEARCH_STRICT_ROBOTS=1` 下 robots 禁则 0 请求；该模式的联网行为在 M4 真站验证中**未**逐引擎证明（单测覆盖 note 文案）。

## 证据（真站慢速验证，2026-11-10）

- 方法：`web_search` 经 MCP 调用，**5 次请求**（bing ×3、baidu ×1、360 ×1），查询为中英混合的“网页抓取/爬虫”主题；请求间隔 **6s**（≥5s），随机抖动 50–500ms 由工具内完成；数据只落 `/tmp`，不入库。
- 结果：**5/5 `OK`**，共 **40** 条结果（bing 10/10/7、baidu 7、360 6）；延迟 p50 **799ms**（min 476 / max 6811，baidu 最慢）。
- 未出现验证码/拦截页；未触发严格模式。
- **相关性未评估**：抽查 top-1 出现明显噪声（如 hao123、输入法页面），故不写“结果质量好”。

## 修改历史

- 2026-11-10：首版（M1–M3 落地后定稿；M2 摘要暂缓）。
