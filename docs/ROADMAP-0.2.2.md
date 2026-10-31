# webctl 0.2.2 路线图：全网搜索（`web_search`）

> 状态：**已与用户对齐（2026-11-01）**，M1 起开工。
> 决策依据：用户 2026-11-01 的六条指示 + 本机实测的引擎可达性/robots 事实。
> 议题：[EPIC #199](https://github.com/Thatgfsj/webctl/issues/199)、[M1 #200](https://github.com/Thatgfsj/webctl/issues/200)、[M2 #201](https://github.com/Thatgfsj/webctl/issues/201)、[M3 #202](https://github.com/Thatgfsj/webctl/issues/202)、[M4 #203](https://github.com/Thatgfsj/webctl/issues/203)。

## 1. 目标

`webctl` 从「单站取回/抽取」扩展到「**按查询词全网搜索**」：

- 新增**第 18 个工具 `web_search`**：输入一个查询词，返回结果卡片（title / url / snippet / rank / engine），正文仍由 `fetch` / `extract` 获取，**不自动抓页面**；
- **Bing 为主引擎**（默认），结果页可附带 Bing 的 LLM 摘要（尽力而为，**必须回原始链接核对**）；
- 可选多引擎（`engines` 参数），由模型自行选择；
- 结果全部按**不可信内容**处理（`trust=untrusted_page_content`），摘要不得当作答案。

**非目标**：验证码/Cloudflare 级对抗、代理或 IP 轮换、自动遍历所有结果页、把搜索结果拼成问答、外部 LLM 调用、内嵌或驱动浏览器。

## 2. 工具形态（已定：新工具，不动现有 `search`）

现有 `search` 是**页面内文本/正则搜索**（query + url|html），保持不变；全网搜索用新名字 **`web_search`**（第 18 个工具）。

| 参数 | 说明 |
|---|---|
| `query` | 查询词（必填） |
| `engines` | 可选数组，默认 `["bing"]`；可选值见 §4 |
| `max_results` | 默认 10，上限 30 |
| `page` | 默认 1（翻页，按各引擎能力） |
| `include_llm_summary` | 默认 true（解析不到就 null，不编造） |
| `include_sources` | 默认 true（摘要/结果的原始链接） |

输出（草案）：

```text
{ query,
  results: [{ rank, engine, title, url, display_url, snippet, engines: ["bing"] }],
  engines: [{ engine, status, count, elapsed_ms, note }],
  llm_summary: { text, sources: [{title, url}], engine, trust, must_visit_sources } | null,
  related: [{ title, url }],
  notes: ["HTML SERP 的合规状态与 robots 事实"] }
```

## 3. 行为约束（用户已确认）

1. **搜索优先、合规其次**：默认走 Bing 的 HTML SERP；不因 robots 禁止而默认拒绝，但**必须如实标注**（结果 `notes` + 文档 + `WEBCTL_SEARCH_STRICT_ROBOTS=1` 严格模式：禁则 0 请求 + `BLOCKED`）；
2. **随机延迟**：每次搜索请求前 `sleep(50..=500ms)`（均匀随机，测试可注入固定种子）；再叠加既有 politeness 限速；
3. **简单反爬（非对抗级）**：浏览器化请求头（UA / Accept / Accept-Language / Sec-Fetch-* / Referer）、会话 cookie 复用（`WEBCTL_BROWSER_CONTEXT` 注入路径）、429 退避；**不做**验证码、Cloudflare 绕过、代理轮换；
4. **LLM 摘要必须回源**：摘要按不可信生成内容处理；返回 `sources` 并置 `must_visit_sources=true`；`actionable.next_steps` 含 `fetch`；解析不到摘要时返回 `null`，绝不编造；
5. **失败语义**：被拦 / 零结果 / 超时 → `DEGRADED` + `degraded_from`（`search-blocked` / `search-empty` / `search-timeout`）+ `actionable`；零结果不返回 OK 空数组。

## 4. 引擎事实（本机实测，2026-11-01）

| 引擎 | 本机可达 | robots 对搜索路径 |
|---|---|---|
| **Bing**（cn.bing.com） | ✅（200，`li.b_algo` ×10 可解析） | `User-agent: *` → `Disallow: /search` |
| Mojeek | ✅ | `Disallow: /search`、`/url` |
| Marginalia（marginalia-search.com） | ✅ | `Disallow: /search`、`/search/` |
| 百度 | ✅ | `User-agent: *` → `Disallow: /` |
| 360（so.com） | ✅ | `Disallow: /s` |
| Yandex | ✅ | `Disallow: /search` |
| DuckDuckGo（html / lite） | ❌ 超时 | lite 的 robots 为 `Allow: /`（唯一合规），但本网络不可达 |
| Brave / Startpage | ❌ 超时 | — |
| 公开 SearXNG 实例 | ⚠️ 429 / 超时 | 实例自身策略 |

**结论**：本网络下不存在「可达 + robots 允许」的免 key 全网搜索通道。因此默认走 Bing（用户决策：搜索优先），并在每份输出与文档中明示这一事实；严格模式供合规要求高的场景。

## 5. 里程碑

| # | 内容 | 议题 | 验收 |
|---|---|---|---|
| M1 | 工具壳 + 结果模型（为 M3 多引擎预留）+ **Bing SERP 解析** + 抖动/礼貌层 + 失败语义 | [#200](https://github.com/Thatgfsj/webctl/issues/200) | 离线 fixture 解析正确；默认只发 Bing 请求；抖动 50–500ms 可断言 |
| M2 | **Bing LLM 摘要 + 相关主题**（必须回源） | [#201](https://github.com/Thatgfsj/webctl/issues/201) | 有摘要→带 sources 且 `must_visit_sources=true`；无摘要→null 不伪造 |
| M3 | `engines` 多引擎 + 聚合/去重/RRF + 分页 | [#202](https://github.com/Thatgfsj/webctl/issues/202) | 逐引擎状态；部分失败仍 OK 且 `search-partial`；全失败 DEGRADED |
| M4 | 文档/工具面/ADR-0008/真站慢速验证 | [#203](https://github.com/Thatgfsj/webctl/issues/203) | 工具面 = 18；文档一致；真站数据不入库 |

## 6. 测试策略

- **全部离线**：手工合成 Bing SERP fixture（脱敏、最小化），覆盖正常 / 零结果 / 验证码页 / 非 HTML；
- **断言抖动**：注入固定种子，断言每次延迟落在 [50, 500] ms；
- **断言合规开关**：`WEBCTL_SEARCH_STRICT_ROBOTS=1` 下 robots 禁止时 **0 请求** + `BLOCKED`；
- **不可信内容**：结果与摘要的 trust 字段、`must_visit_sources`、`actionable` 含 `fetch`；
- **真站验证（M4，需授权）**：3–5 个固定查询，随机抖动 50–500ms、间隔 ≥5s、总请求 ≤10；只把结论写进文档，原始 HTML 与结果数据**不进仓库**。

## 7. 风险登记

1. **robots/ToS 冲突**：默认开启 Bing HTML SERP 是用户明确决策（搜索优先、合规其次）；文档必须持续如实标注，不宣称「合规」；
2. **摘要可能不可得**：cn.bing.com 对未登录/无 JS 请求通常不渲染 Copilot 摘要；M2 尽力解析，解析不到就 `null`，必要时提示用浏览器上下文注入 cookie；
3. **选择器易变**：Bing DOM 会变；解析器用多组选择器 + 失败即 `search-parse-failed`（不返回空成功）；fixture 固定回归；
4. **合规压力**：严格模式与文档是唯一缓解；不做任何绕过升级。

## 8. 与其它议题的关系

- 复用：`PoliteFetcher`（robots/限速/429）、浏览器上下文注入（cookie/UA）、`actionable` 原因码表、现有 HTML 解析栈；
- 安全前置：[#191](https://github.com/Thatgfsj/webctl/issues/191)（SSRF 防护）——`web_search` 的结果 URL 校验复用其实现（M1 先做最小 url/http(s) 校验，完整实现随 #191）；
- 0.2.1 后的质量议题（#187 / #188 / #190）不在本迭代内。
