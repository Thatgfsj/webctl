# SPIKE-201：Bing LLM 摘要与「相关主题」的可复现性验证

> 状态：**已结题（2026-11-23）——判定「不可稳定复现」**，M2 维持暂缓，**产品代码零改动**。
> 议题：[#201](https://github.com/Thatgfsj/webctl/issues/201)（[ROADMAP-0.2.2 §5](ROADMAP-0.2.2.md) 的 M2）；决策基线：[ADR-0008 决策 8](adr/0008-search-channel.md)。
> 影响面：`web_search` 的 `llm_summary` **仍恒为 `null`**；工具面仍 **18** 个；`related` 的既有实现不动（见 §3.5 的一处新观测，建议另开议题核实）。

| 项 | 值 |
|---|---|
| 日期 | 2026-11-23 23:17–23:28（Asia/Shanghai） |
| 方法 | 本机网络 `curl` 直连 Bing HTML SERP（**无 JS 引擎**、匿名、未登录） |
| 请求量 | **33** 次真实 HTTP 请求（含 1 次产品自身 `webctl fetch` 交叉复核）；**25** 份可评估的 SERP 级样本（另有首页 1、302 体 2、404 1） |
| 命中率 | LLM 摘要区块 **0 / 25（0%）**；`li.b_ans` 元素 **0**；渲染出的「相关主题」**0**；`#b_context` **24/24 为空** |
| 结论 | **不可稳定复现**：静态 HTML 里根本不存在答案容器，摘要与相关主题都由客户端 JS 注入 |
| 数据去向 | 原始 HTML 只落 `/tmp/bing201/`（易失、**不入库**）；本文只记录方法、次数、脱敏后的最小结构 |

---

## 1. 判定门槛

按议题约定：

- 先做可复现性验证：**≥5 次**、**不同查询**、**间隔请求**（≥5s）真实取回；
- 若 **≥3/5 次出现且结构一致** → 实现 M2（`data.llm_summary = { text, sources[], engine, trust, must_visit_sources: true }`、`actionable.next_steps` 含 `fetch`、解析不到返回 `null` 不报错、摘要按不可信内容、合成 fixture + 离线测试、工具面仍 18）；
- 否则**不改产品代码**，只写本文件，结论为「维持暂缓 / 不能声称已实现」。

**实际：0/25 命中，门槛未达到。**

---

## 2. 观测方法

### 2.1 取回姿态

| 维度 | 取值 |
|---|---|
| 客户端 | `curl 8.14.1`（HTTP/2），**不执行 JS** |
| UA | `Chrome/131.0.0.0`（桌面）；其中 1 次为 iPhone Safari UA 作对照 |
| 头 | `Accept`(html/xhtml)、`Accept-Language`（zh-CN / en-US 两套）、`Upgrade-Insecure-Requests: 1` |
| 市场 | `setlang=zh-CN`、`mkt=zh-CN`、`mkt=en-US&setlang=en`、`form=QBLH` |
| 站点 | `www.bing.com/search`、`cn.bing.com/search`（会 302 回 www）、`www.bing.com/copilotsearch` |
| 会话 | 两组：A 组全新无 cookie；B 组先取 `https://www.bing.com/` 拿 cookie（`_HPVN/MUID/SRCHD/SRCHHPGUSR/SRCHUID/SRCHUSR/_SS`）后复用 |
| 间隔 | 每次请求间隔 **8–12s**（≥5s，礼貌且避免触发限流） |
| 落盘 | `/tmp/bing201/{q,a,b,c,d,e}*.html`（**不入库**） |

### 2.2 请求轮次

| 轮次 | 查询数 | 请求数 | 姿态 | 结果 |
|---|---:|---:|---|---|
| R0 探测 | 1 | 1 | 桌面 UA，`q=test` | 200，结构正常 |
| R1 首轮 | 2 | 2（另有 4 次因 URL 未编码在本地被 curl 拒绝，未发出） | 桌面 UA，zh 头 | 200 ×2 |
| R2 新鲜会话 | 8 | 9 | 首页预热 1 次取 cookie + 8 个查询 | 200 ×9（其中 1 份为「无结果」页） |
| R3 cookie 会话 | 5 | 5 | 先访问首页取 cookie 再查询 | 200 ×5 |
| R4 市场/参数 | 6 + 2 | 8 | `mkt=en-US`、`form=QBLH`、`showconv=1` | 200 ×4；`showconv=1` 两次均 **302** → `copilot.microsoft.com`，`-L` 复取 2 次 |
| R5 专用路由 | 3 | 3 | `/copilotsearch`（en/zh）、移动 UA | 200（SPA 壳）、**404**、200（移动版 5 条） |
| R6 站点/热门 | 4 | 4 | `cn.bing.com`、`chatgpt`、`2026世界杯` + cookie | 200 ×4（其中 1 份「无结果」页） |
| R7 产品交叉复核 | 1 | 1 | 本仓库编译出的 `webctl fetch`（产品 UA/头，默认配置） | 200；`final_url` 落到 `cn.bing.com`，结构与 curl 组一致（见 §3.1 末行） |

合计 **33** 次真实请求、**11 个不同查询**；两套 `Accept-Language`、两组会话、三种站点/路由；并用产品自身二进制做了一次交叉复核。

### 2.3 判定口径（避免把 CSS 误当区块）

`b_ans` 这个字符串在 Bing 页面里**同时**出现在两类位置，必须区分：

- **元素**：`<li class="b_ans …">` / `<div class="b_ans …">` —— 这才是答案区块；
- **内联 CSS 与 JS 字符串**：`#b_results>.b_ans:nth-child(n+7) …`、`#b_content .b_ans` —— 只是样式规则。

因此判定脚本用的是**元素正则**（`<tag … class="… b_ans …"`），而不是 `grep b_ans`。纯文本计数在本轮会给出 **30–38 次假阳性**（见 §3.3）。

---

## 3. 结果

### 3.1 汇总

| 指标 | 观测值 |
|---|---|
| LLM 摘要区块（答案 + 「来源」链接列表） | **0 / 25 样本** |
| `li.b_ans` / `div.b_ans` 元素 | **0 / 29 份落盘响应** |
| `b_sydConv*` / `b_topborder` / `b_summary` / `b_rich` 元素 | **0** |
| 「相关主题」区块（渲染出的） | **0**；`#b_context` **24/24 为空**（见 §3.5） |
| 「相关搜索」 | 仅以 **JS 本地化字符串**出现（`"RelatedSearches":"相关搜索"`），无渲染区块 |
| 隐藏的 Copilot 外壳 `#b_copilot_search_container` | 15 / 25 样本存在，**只有免责声明、无正文无来源**（见 §3.4）；zh 10 份、en 5 份 |
| 自然结果 `li.b_algo` | 22 / 25 样本有结果（20 份 ×10 条、1 份 ×7、1 份 ×5）；2 份「无结果」页、1 份 SPA 壳 |
| 免责声明（zh「内容使用 AI 生成。」/ en「Content was generated with AI.」） | 15 个含外壳样本 **15/15** 均带，**全部**位于隐藏外壳内 |
| 产品自身取回（`webctl fetch`，R7） | 200 / `li.b_algo` 10 条 / `b_ans` 元素 0 / `#b_context` 空 / 无隐藏外壳；**与 curl 组同构** |

**命中率 0%**（0/25 样本、0/33 请求）。既没有「摘要 + 来源」区块，也没有 `b_ans` 元素。产品自身的取回路径（R7）与 curl 组同构，说明这不是「curl 与产品 UA 差异」造成的假阴性。

### 3.2 观测到的最小静态结构（脱敏）

有结果的 SERP，`#b_results` 的子项**只有三类**：

```text
<ol id="b_results">
  <li class="b_algo">…</li>      × 7–10   ← 自然结果（标题 / 链接 / 摘要 / cite）
  <li class="b_msg b_canvas">…</li>       ← 「无更多结果」类提示
  <li class="b_pag">…</li>                ← 分页
</ol>

<aside aria-label="更多结果"><ol id="b_context"></ol></aside>   ← 空
<aside aria-label="浏览更多"><ol id="b_dynRail"></ol></aside>   ← 空
```

即：**静态 HTML 里没有为「模型生成摘要」预留任何已渲染容器**；侧栏 `b_context`（相关搜索/相关主题的宿主）也是空的，靠 JS 填充。

### 3.3 反证：`b_ans` 只存在于 CSS

在 `/copilotsearch` 与移动版响应的原始 HTML 中，`b_ans` 字符串出现 30–38 次，逐一查看上下文，**全部**是内联样式表：

```css
#b_results>.b_ans:not(.b_top):nth-child(n+5) .rqnaContainerwithfeedback …
#b_results>.b_ans:nth-child(n+7) .b_rs:not(.pageRecoContainer) …
#b_context .b_ans,#b_rrat_cont .b_ans,.b_expando .b_ans{…}
```

但同批页面里 `<… class="… b_ans …">` **元素数为 0**。这说明 Bing 仍保留答案卡的样式，只是**不再把答案写进首屏 HTML**。任何「`grep b_ans` 命中即认为有摘要」的实现都会误判。

### 3.4 隐藏的 Copilot 外壳（有壳无内容）

15 份样本里存在这个容器，脱敏后最小结构：

```html
<div id="b_copilot_search_container" class="b_hide">
  <div class="b_copilot_search">
    <div class="b_inlineChatInitWrapper">
      … <div class="b_cs_disclaimer"><span>内容使用 AI 生成。 </span><a>了解更多</a></div>
    </div>
  </div>
</div>
```

- 容器带 `b_hide`（隐藏），内部**没有摘要正文、没有来源列表、没有 `<iframe>`**；
- 逐段取文本只得 `内容使用 AI 生成。 了解更多`（en-US 页面为 `Content was generated with AI. Learn more`）；
- 结论：这是**入口/免责声明外壳**，正文由前端脚本另行请求。纯 HTML 取回永远拿到空壳。这解释了议题里「缓存命中不可稳定复现」的现象：不是缓存时机问题，而是**静态响应里根本没有内容**。

### 3.5 「相关主题」：侧栏恒空

24 份样本含 `<ol id="b_context"></ol>`，**24/24 内部为空**；`b_dynRail` 同为空的注入点。真正的「相关搜索/人们还搜索了」链接要在浏览器里跑完 JS 才会出现。

> 附带观测（**本 spike 不改代码**）：M1 的 `related` 解析选择器是 `#b_context a[href], #b_rs a[href]`。按本轮证据，匿名无 JS 取回时这两个容器都没有 `a`，真站 `related` 很可能恒为 `[]`。这与 ADR-0008「`related` 已随 M1 落地」的表述存在偏差，**建议另开议题核实**（需要真站慢速验证或浏览器上下文注入），不在本 spike 范围内动手。

### 3.6 不稳定证据（同一网络的另外三类现象）

| 现象 | 观测 |
|---|---|
| `showconv=1` | 302 → `copilot.microsoft.com/?…&q=…`（SPA，HTML 体 188/205 字节），不是 SERP 摘要 |
| `/copilotsearch` 路由 | `mkt=en-US` 200（213KB，**SPA 外壳**，无答案、无来源）；`mkt=zh-CN` **404** |
| `cn.bing.com` | 302 回 `www.bing.com/search`；同参数下 1 次返回「无结果」页（`li.b_no`，0 条） |
| 移动 UA | 返回 5 条**与查询无关**的越南语结果（地区/个性化错配）——同网络下结果集合本身就不稳定 |

这些现象说明：即使只做「结果解析」，同一网络、同一查询的响应也可能在「10 条 / 0 条 / 无关结果」之间摆动；把「摘要是否出现」当作可断言的产品行为，前提并不成立。

---

## 4. 判定

**不可稳定复现 → 维持暂缓（M2 不实现）。**

1. 命中率 0/25（0%），远低于「≥3/5 且结构一致」的门槛；
2. 不是「偶尔命中」，而是**静态 HTML 里没有容器**：`li.b_ans` 元素 0，`#b_context` 24/24 空，唯一 AI 相关物是隐藏的免责声明外壳；
3. 因此**不能声称已实现** `llm_summary`；`web_search` 保持 `llm_summary = null`，文档、instructions、SKILL 继续按 ADR-0008 决策 8 表述；
4. 用合成 fixture 为「摘要」写离线测试**不可信**：没有真实结构可仿，只能仿造一个不存在的 DOM，等于用测试证明一个假契约。

本轮**未改动任何产品代码/测试/fixture**，工具面仍 18。

---

## 5. 若将来要做，需要的前置条件

### P0（缺一不可）

1. **找到摘要的静态载体**：要么接受「需要执行 JS」（与 ADR-0008「不内嵌浏览器」冲突，需用户重新决策），要么找到前端实际调用的答案接口（隐藏外壳对应的 XHR），并证明它**在无登录、无 JS 会话下可稳定返回**；
2. **可复现的触发条件**：给定市场/locale/查询类型/会话态，连续 **≥3/5** 次出现且结构一致；仅「热门查询偶尔出现」不足以支撑实现；
3. **合成 fixture 的来源合法**：按观测到的**结构**手写最小 DOM，不得提交真站 HTML；同时必须能证明该 fixture 与真站结构一致（否则测试无意义）；
4. **契约与合规复核**：`trust=untrusted_page_content`、`must_visit_sources=true`、`actionable.next_steps` 必含 `fetch`；robots 对 `/search` 的 `Disallow` 事实继续在 `notes` 如实披露；
5. **降级语义**：解析不到 → `llm_summary = null` 且**不报错**、不降级整体状态；摘要存在但来源缺失 → 视为解析失败而非「有摘要无来源」。

### P1（建议）

1. 先做**观测脚本**（本 spike 的 `/tmp` 脚本可作雏形）：固定查询集、固定间隔、元素级判定，输出命中矩阵；脚本入库、**数据不入库**；
2. 若考虑走 `/copilotsearch` 路由：需单独评估 SPA、分市场 404、以及它与 SERP 的关系（本轮 en 200 / zh 404）；
3. 复核 §3.5：为 `related` 补一次真站观察（浏览器上下文注入或 `WEBCTL_BROWSER_CONTEXT`），确认它是否真能取到；
4. 增加一条**回归断言**：任何「摘要解析」实现都不得仅凭 `b_ans` 字符串命中就判定成功（本轮 30–38 次假阳性即为反例）。

---

## 6. 复现方法（不含真实内容）

```bash
# 单次取回；间隔 ≥8s；数据只落 /tmp
UA='Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36'
curl -sS -G -o /tmp/bing201/probe.html --max-time 30 \
  -H 'Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8' \
  -H 'Accept-Language: zh-CN,zh;q=0.9,en;q=0.8' -A "$UA" \
  --data-urlencode 'q=<QUERY>' 'https://www.bing.com/search?setlang=zh-CN'
```

判定（**元素级**，不要 `grep b_ans`）：

```python
import re
h = open('/tmp/bing201/probe.html', encoding='utf-8', errors='replace').read()
ans_el   = re.findall(r'<[a-zA-Z][^>]*class="[^"]*\bb_ans\b[^"]*"', h)   # 必须是 0 才算「没有」
ans_css  = len(re.findall(r'\.b_ans', h))                                   # CSS 命中，不构成证据
ctx_empty = re.findall(r'<ol id="b_context">\s*</ol>', h)                   # 空侧栏
shell     = 'b_copilot_search_container' in h                               # 隐藏外壳（有壳无内容）
print(len(ans_el), ans_css, len(ctx_empty), shell)
```

本轮实测输出形态：`0  30~38  1~2  True`（答案元素 0、CSS 命中数十、侧栏空、外壳存在）。

---

## 7. 未验证 / 局限

1. **未验证登录态 + 真浏览器 JS 场景**：本 spike 的能力边界就是「无 JS、匿名」（与产品一致）；不做验证码/风控对抗；
2. **未验证其它出口 IP / 地区**：本轮出现越南语无关结果，说明网络地区可能影响响应；结论只代表**本机网络**；
3. **未评估摘要质量**：0 命中，无从评估；
4. **样本量**：25 份 SERP 样本、11 个查询，足以判定「不能稳定复现」，但不足以证明「全网络永不出现」；
5. `/tmp/bing201/` 为易失数据，脚本与 HTML 均不入库。

---

## 8. 变更历史

- 2026-11-23：首版（33 请求 / 25 样本 / 0 命中；判定不可稳定复现，M2 维持暂缓）。
