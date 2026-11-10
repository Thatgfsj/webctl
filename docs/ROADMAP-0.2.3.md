# webctl 0.2.3 路线图：搜索降噪 + 诚实降级与凭据接力

> 状态：**已与用户对齐（2026-11-10）**，由 lead 规划，子代理分发实现。
> 总议题：[EPIC #220](https://github.com/Thatgfsj/webctl/issues/220)；子议题 [M1 #221](https://github.com/Thatgfsj/webctl/issues/221)、[M2 #222](https://github.com/Thatgfsj/webctl/issues/222)、[M3 #223](https://github.com/Thatgfsj/webctl/issues/223)、[M4 #224](https://github.com/Thatgfsj/webctl/issues/224)、[M5 #225](https://github.com/Thatgfsj/webctl/issues/225)、[M6 #226](https://github.com/Thatgfsj/webctl/issues/226)。
> 前置：0.2.2（`web_search` 多引擎）已定版待打 tag；0.2.3 基于其主干开工。

## 1. 目标

两个方向，共 4 个子方向、6 个里程碑：

- **方向一（搜索降噪）**：去广告 + 排除操作符 + 紧凑性，让 `web_search` 返回给 LLM 的结果更干净、更省 token，且**可诊断**（不静默删结果）。
- **方向二（诚实降级 → 自主回退）**：webctl 不集成 Playwright，只把"为什么降级、是否适合回退、已经试过什么"结构化上报；提供宿主可声明的 fallback 能力；导出 Playwright 可用的 `storage_state`，让登录态可以接力。

**非目标**：webctl 内嵌或主动调用 Playwright/浏览器；验证码/Cloudflare 对抗；代理轮换；为搜索引擎做 LLM 摘要（ADR-0008 已暂缓 M2）。

## 2. 方向一：搜索降噪

### 2.1 去广告（声明式规则 + 冻结样本）
- 双层信号：① 显式广告标签文本（「广告」/「Ad」/「Sponsored」/「赞助商链接」）；② URL 中转域检测（`bing.com/ck/a`、`baidu.com/baidu.php?`、`so.com/Link?`，结果 href 与显示 href 不一致）。跳转域比 class 稳定，优先信任第 ② 层。
- 规则以声明式表（数据）存放，不写死在解析函数里；每条规则带 `confidence: high|low`。
- **高置信** → 剔除，计入 `filtered_ads: N`；**低置信** → 保留并标 `is_ad: true`。
- 冻结 HTML fixture（脱敏）进离线测试，作为"广告样本回归"；规则变更必须跑过 fixture。

### 2.2 排除操作符
- `web_search` 新增 `exclude`（如 `["-tutorial", "site:csdn.net"]` 或直接 `-site:csdn.net` 形式，二选一在子议题内定）。
- 透传给引擎（Bing/Baidu/360 原生支持 `-term`）。
- envelope 回显 `applied_operators`。
- **生效校验**：对比带/不带操作符的结果数；引擎忽略（典型为 Baidu）时打 `DEGRADED` + `degraded_from: search-operator-ignored`，并给出 `actionable`。

### 2.3 紧凑性
- **同域聚合**：同一域名多条结果折叠为一条主结果 + `same_domain_count`（保留完整列表可选 `expand_domains=true`）。
- **snippet 截断**：默认 300 字符（参数可调），正文仍走 `fetch/extract`。
- 复用既有 URL 归一化去重（utm/fbclid）。

## 3. 方向二：诚实降级 → 自主回退

### 3.1 结构化 fallback_hint
在 `meta.actionable` 上扩展（向后兼容，新增字段均可选）：

```json
"fallback_hint": {
  "playwright_suitable": true,
  "signals": ["#root empty", "script[src*=chunk]"],
  "embedded_data_tried": true,
  "js_engine_suitable": false
}
```

- `playwright_suitable`：由信号判定（空壳 body、框架根节点、外链 chunk、hydration 标记）。
- `embedded_data_tried`：内嵌数据抽取是否已尝试且无果。
- `js_engine_suitable`：依赖外链库时为 false（内联引擎也救不了）。
- **纯建议，webctl 绝不主动调用**。

### 3.2 caps 声明 fallback 能力
- 宿主通过配置文件/环境变量声明"本环境存在 Playwright MCP 工具 X"。
- caps 工具返回该声明；`next_steps` 只"指向"它，webctl 零依赖。

### 3.3 凭据接力（重点）
- webctl 会话中的 cookie / storage 可导出为 **Playwright `storage_state` JSON** 格式（`cookies[]` + `origins[]`，字段名与 Playwright 对齐）。
- 导出需脱敏策略：默认仅导出目标域；导出文件权限 0600；不进日志。
- 提供 CLI/工具入口导出；文档给出"webctl 登录 → 导出 → Playwright 复用"的最小示例。

### 3.4 SKILL.md 决策树（文档）
`embedded_data → js_engine（若启用）→ fallback_hint.playwright_suitable 时由 agent 决定回退`。

## 4. 里程碑

| # | 内容 | 方向 | 验收 |
|---|---|---|---|
| M1 [#221](https://github.com/Thatgfsj/webctl/issues/221) | 去广告规则表 + 冻结 fixture（Bing/Baidu/360）+ `filtered_ads` / `is_ad` | 一 | 离线 fixture 全绿；高置信剔除、低置信打标；`filtered_ads` 数量正确；误杀样本为 0 |
| M2 [#222](https://github.com/Thatgfsj/webctl/issues/222) | `exclude` 排除操作符 + `applied_operators` + 生效校验 | 一 | 透传参数正确；Baidu 忽略路径触发 `search-operator-ignored`；不回归 0.2.2 行为 |
| M3 [#223](https://github.com/Thatgfsj/webctl/issues/223) | 同域聚合 + snippet 截断 | 一 | 同域折叠计数正确；默认截断 300；`expand_domains` 可还原完整列表 |
| M4 [#224](https://github.com/Thatgfsj/webctl/issues/224) | `fallback_hint` 结构化（fetch/extract DEGRADED） | 二 | 原因码表登记（不漏）；JS 页 `playwright_suitable=true`；纯静态页不出现 hint |
| M5 [#225](https://github.com/Thatgfsj/webctl/issues/225) | caps fallback 能力声明 + SKILL.md 决策树 | 二 | 配置声明可读出；未声明时 `next_steps` 不指向外部工具 |
| M6 [#226](https://github.com/Thatgfsj/webctl/issues/226) | storage_state 导出（脱敏 + 0600） | 二 | 导出 JSON 结构符合 Playwright 规范（离线校验）；不含非目标域 cookie；权限断言 |

依赖：M1 → M2 → M3（同一 web_search 工具面，串行）；M4 → M5（M5 依赖 M4 的信号）；M6 独立，可与方向一并行。

**当前进度（2026-11-10）**：M1 ✅（PR #230 `4961ad8`）、M2 ✅（PR #233 `60bfafc`）、M3 ✅（PR #234 `fd04fea`）、M4 ✅（PR #229 `7db617a`）、M5 ✅（PR #232 `ec7fd31`）、M6 ✅（PR #231 `3d22512`）。全部里程碑已合并，版本已定版为 **0.2.3**（待打 tag）。


## 5. 测试策略

- **全部离线**：脱敏 SERP fixture（含广告样本）、合成 JS 空壳页 fixture、合成 storage 数据。
- **规则回归**：广告规则改动必须通过 fixture 套件（CI 中新增一条扫描，类比"防降级路径漏登记"）。
- **不可信内容**：结果 trust 字段不变；广告标记不改变 trust。
- **真站验证（需授权）**：仅在里程碑结尾做一次慢速抽查，数据不入库，结论写文档。

## 6. 风险

1. **广告标记是移动靶**：以跳转域为主信号，fixture 固定回归，改版即更新 fixture。
2. **误杀**：宁漏勿杀；误杀样本计入回归，出现即阻断。
3. **Baidu 排除操作符不可靠**：靠生效校验 + DEGRADED 诚实上报，不承诺。
4. **凭据泄漏**：storage_state 含敏感 cookie；默认仅目标域、0600、不进日志、文档显著提示。
5. **fallback_hint 误导 agent**：信号不足时宁可 `playwright_suitable=false` 并说明原因。

## 7. 与其它议题的关系

- 复用：0.2.2 的 `web_search` 引擎层、RRF 聚合、`actionable` 原因码表（#124）、浏览器上下文注入（ADR-0006）。
- 安全前置：#191（SSRF 防护）—— 结果 URL 校验复用其实现。
- 不在本迭代：#192（js_render=auto）、#193（schema 抽字段）等，保留在 backlog。
