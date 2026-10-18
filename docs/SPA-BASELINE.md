# SPA 样本集与「有效信息获取率」基线（议题 #119 / EPIC #113）

**为什么要有这个文件**：没有基线就无法证明 0.1.2 真的改善了。这里的数字都是**实测**的，
并写明来源与日期——真站会变，必须能复现"当时是这样"。

## 1. 指标定义

**不是** HTTP 200。对每个样本逐通道记录是否拿到可用信息：

| 通道 | 怎么算 |
|---|---|
| **正文** | `extract` 用内容选择器（见各表）命中数 ≥ 1 **且**信封不是降级码 |
| **内嵌结构化数据** | `embedded_data` 的 `data.entries` ≥ 1（结果**落盘**时以 `data.saved_to` 指向的完整信封里的 `entries` 计，见 §3 的复现必读） |
| **元数据** | HTML 头部有 RSS/JSON-LD 等可发现线索（本版只记录"有无"，发现能力见 #117） |
| **无** | 三条通道都没拿到 —— **即使 HTTP 200 也记 `none`** |

`有效信息获取率` = 至少一条通道拿到可用信息的样本数 / 样本总数。

**两条纪律**（沿用本仓既有做法）：

1. 真站不可达或取不到 → **如实记"未取到"**，不得用跳过或删除样本掩盖；
2. 本文件与 `crates/webctl-web/tests/spa_baseline.rs` **成对更新**：改行为必须同时改测试基线并在此说明原因。

## 2. 离线样本（进 L0，可复现）

样本在 `crates/webctl-web/tests/fixtures/spa/`，由 `cargo test -p webctl-web --test spa_baseline` 实测钉住。

| 样本 | 正文字符 | 内嵌条数 | 内嵌来源 | 判为依赖 JS | RSS 线索 | obtained |
|---|---|---|---|---|---|---|
| nextjs（JSON-LD + `__NEXT_DATA__`，HTML 空壳） | 0 | **2** | `ld+json`+`script-json` | 是 | 无 | **embedded** |
| nuxt（`window.__NUXT__` 内联状态，HTML 空壳） | 0 | **1** | `inline-state` | 是 | 无 | **embedded** |
| 纯 CSR 空壳（无任何数据） | 0 | 0 | – | 是 | 无 | **none** |
| 静态文章 + JSON-LD（正文在 HTML 里） | 128 | 1 | `ld+json` | 否 | 无 | **content** |
| 静态博客列表 + RSS 发现链接 | 119 | 0 | – | 否 | **有** | **content** |

离线样本的 `有效信息获取率` = **4/5**（纯 CSR 空壳无任何数据可拿，诚实记 `none`）。

> 写样本时踩到的一个真实边界：`window.__NUXT__={"data":[…]}` 是 **JSON** 才能抽出；
> 写成 JS 对象字面量（键不加引号）**不会**被硬猜（EMBED-05）。样本里已注明。

## 3. 真站样本（L1 手动通道，会抖）

实测时间：**2026-10-17/18**；版本：`main` @ `cabc914`（v0.1.2 开发中，已含 #114 内嵌数据、#115 可行动降级）。

复现协议（`webctl-mcp` 的 stdio JSON-RPC，逐项 `tools/call`）：

```text
fetch         {"url": <URL>}                       # 看 code / meta.degraded_from
extract       {"url": <URL>, "selector": <SEL>}    # 看 code / matches 条数
embedded_data {"url": <URL>}                       # 看 code / data.entries 条数
```

> **⚠️ 复现必读（我在这里踩过一次坑）**：结果超过内联上限时，`tools/call` 的**内联信封是落盘占位**
> ——`data = { bytes, inline: false, note, saved_to, tool }`，**没有 `entries`/`matches` 字段**。
> 只看内联字段会把"有数据"误记成 `0`。必须顺着 `data.saved_to` 读完整信封。
> （本表 `nuxt.com` 就是这种情况。）

| 站点 | 类型 | `fetch` | `extract`（选择器） | `embedded_data` | obtained | 0.1.1 时的状态 |
|---|---|---|---|---|---|---|
| `quotes.toscrape.com/js/` | 纯 CSR 空壳 | **DEGRADED**/`js-render` | DEGRADED/`js-render`，0 条（`div.quote`） | OK，0 条 | **none** | 同样 `none`（#113 的痛点） |
| `quotes.toscrape.com/` | 服务端渲染 | OK | OK，**10 条**（`div.quote`） | OK，0 条 | **content** | 同（L1 锚点） |
| `nextjs.org` | Next.js | OK | OK，1 条（`main`） | OK，**1 条**（`ld+json`，1940 B） | **content + embedded** | 正文通道同；**内嵌数据拿不到**（当时无该工具） |
| `tailwindcss.com` | 内容站 | OK | OK，**0 条**（`main`） | OK，0 条（扫了 187 个 script） | **none** | 同 |
| `nuxt.com` | Nuxt | OK | OK，1 条（`main`） | OK，**3 条**（`ld+json` 1836 B + `__NUXT_DATA__` 171264 B + `inline-state` 2 B；163 KB **落盘**） | **content + embedded** | 内嵌数据拿不到（当时无该工具） |

> `tailwindcss.com` 的 `scanned_scripts` 会在 186～187 之间抖（站点本身变化），不影响 `obtained`。

真站 `有效信息获取率`（至少一条通道）：**4/5**（`tailwindcss.com` 如实记 `none`）。

**未取到的样本**：无（五个站点在实测时全部可达）。

## 4. 0.1.2 相对 0.1.1 的实际变化（不许夸大）

| 变化 | 事实 |
|---|---|
| **新增内嵌数据通道** | 真站 5 个样本里 **2 个**（`nextjs.org` 的 `ld+json`、`nuxt.com` 的 `ld+json`+`__NUXT_DATA__`+`inline-state`）从"拿不到结构化数据"变为拿到；离线样本 2/5 靠这条通道拿到内容 |
| **降级可行动**（#115） | JS 空壳页的降级信封现在带 `meta.actionable`（原因码 + 证据 + 建议通道），调用方知道"还能试 `embedded_data`" |
| **JS 空壳仍然拿不到内容** | 引擎**已接线**（#128 步骤 2，默认关闭）；开启后本页仍 `none`，原因是它依赖**外链** jQuery（`ReferenceError: $ is not defined`），见 §5 与 #137。本条项要等 0.1.2 的引擎 PR |
| **总体获取率未变** | 真站仍是 4/5：本版改善的是**通道覆盖**（内嵌数据通道 **0/5 → 2/5**），不是总数 |

## 5. 0.1.2 完成后的对比（引擎接线后实测，2026-10-18）

引擎已接线（`js_render` 开关，**默认关闭**；见 [CONFIGURATION §5](CONFIGURATION.md)）。实测：

- **离线夹具（仓库自己的 JS 空壳样本）**：开关开启后 `extract div.quote` 命中 **10 条** ✓（内容原本只在 `document.write` 里）；
- **真站 `quotes.toscrape.com/js/`**：开关开启后**仍未取到** ✗，但**原因明确且如实报告**：
  该页的内联脚本依赖 **jQuery**，而 jQuery 是**外链脚本**（本版按设计不执行外链），
  于是渲染失败并降级为 `degraded_from=js-render-failed`，`actionable.detected` 给出
  `ReferenceError: $ is not defined`——**不是**"成功但空"；
- 结论：本版让"内容由**内联**脚本生成"的页面可取；让"依赖外部 JS 库"的页面**如实失败**。
  后者的处理（取同源外链脚本再执行）已立议题 [#137](https://github.com/Thatgfsj/webctl/issues/137)。
- `tailwindcss.com` 是否仍为 `none`（若变，说明改变了什么）；
- 离线样本基线的 diff（`spa_baseline.rs` 的断言必须显式更新）；
- 引擎带来的体积/耗时变化（发布资产与 `ldd` 证据见 `docs/RELEASE.md`）。

## 6. 已知边界

- 真站数字会漂移（站点改版、地区差异、反爬），所以每次更新都**必须带日期**；
- `extract` 的降级判定与**选择器是否命中**有关：用 `body` 这类"一定命中"的选择器会绕过 JS 空壳判定（实测 `body` → OK），
  所以基线统一用**内容选择器**；
- 元数据通道本期只记录线索（RSS/JSON-LD 是否在头部），真正的发现与利用见 #117；
- `doc_text`/`download`/`json_query` 在 JS 页上的"静默 OK"是另一个已立项的问题（#125），不在本基线口径内。
