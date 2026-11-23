# 自适应选择器跟踪（Spike #195）

| 项 | 值 |
|---|---|
| 议题 | [#195](https://github.com/Thatgfsj/webctl/issues/195)（scope: extract / type: spike） |
| 日期 | 2026-11-23 |
| 结论 | **有条件可行，但只能作为显式 opt-in，不能进入默认 extract**：受控改版样本 6/6 重定位；重复 DOM 正确判歧义；内容与结构同时大改判 not_found |
| 原型 | [crates/webctl-web/src/extract/adaptive.rs](../crates/webctl-web/src/extract/adaptive.rs)（研究模块，未接入 dispatch / CLI / MCP） |
| 样本 | [crates/webctl-web/tests/fixtures/adaptive/](../crates/webctl-web/tests/fixtures/adaptive/) |
| 测试 | [crates/webctl-web/tests/adaptive.rs](../crates/webctl-web/tests/adaptive.rs)（10 个离线用例） |
| 后续 | 若实现，先采纳 [ADR-0009 草案](adr/0009-adaptive-selectors.md) 的诚实边界 |

---

## 1. 结论先行

1. **能找回来，但样本是受控的。** 同一站点的 v1/v2 两版 HTML 里，v2 做了标签替换（div → article）、class 重命名、包裹层插入与条目换序；5 条标题链接 + 1 段正文全部被重定位（6/6）。这不是线上命中率，只是"在文本与祖先结构基本保留时，启发式确实有效"的存在性证据。
2. **诚实标注是可实现的，而且必须默认开启。** 精确选择器仍命中时直接返回 exact，不跑相似度；零命中时只返回候选，标 matched_by=similarity、confidence、逐条 evidence、requires_confirmation=true。输出 JSON 里相似度结果永远不会伪装成 selector。
3. **歧义与指纹不足必须拒绝，而不是硬猜。** 桌面端 + 移动端重复同一份 DOM 时，前两名分数完全相同，标 ambiguous；只有 tag/祖先路径的锚点标 insufficient_fingerprint。内容与结构大改的样本上，最高分低于阈值，标 not_found。
4. **代价可控，但阈值不是校准出来的。** 默认 60% 置信度、0.06 分差、Top 3 候选、5000 节点扫描、200 个候选、50 ms 预算；本次 fixture 上单次调用远低于预算（见 §4.4）。阈值是启发式，不是概率，不能对外宣称"60% 概率找对"。
5. **不建议做成"自动修复"。** 建议只在调用方显式提供上一版指纹/上下文时启用；结果永远是候选，零命中仍然是合法的 OK 零命中。若实现，先落 ADR-0009。

---

## 2. 范围与非目标

### 2.1 本 spike 覆盖

- 给一个选择器和上一版 HTML，记录**单元素**的轻量指纹；
- 在当前 HTML 里先跑精确选择器；零命中时按相似度找候选；
- 输出候选、置信度、证据、状态；用同一站点改版前后两版 HTML 做离线命中率测试；
- 给出阈值、候选数、墙钟上限与"不值得做"的边界。

### 2.2 非目标

- 不改变默认 `extract` 行为：`select_matches`、CLI、MCP 的 18 个工具完全不知道这个模块存在；
- 不做跨请求状态存储、不做指纹持久化、不做自动写回；
- 不做多锚点/集合对齐（一个选择器命中 N 个元素时只取第一个）；
- 不做 JS/SPA 渲染后 DOM 的跟踪；
- 不引入模型、嵌入、外部服务或新依赖。

---

## 3. 原型设计

### 3.1 元素指纹

| 字段 | 说明 | 稳定性来源 |
|---|---|---|
| `tag` | 小写标签名 | 结构 |
| `stable_id` | 过滤后的 id | 语义 id |
| `stable_classes` | 过滤、排序、去重后的 class token | 语义 class |
| `text` | 元素后代文本，折叠空白并截断到 200 字符 | 内容 |
| `before` / `after` | 前/后一个兄弟元素的文本，各截断 80 字符 | 邻近内容 |
| `ancestor_tags` | 从根到父元素的标签链 | 层级路径 |
| `depth` / `sibling_index` | 祖先层数与同级元素索引 | 位置 |

指纹可序列化。未来若做跨请求跟踪，调用方可以自己保存这个 JSON；本 spike 不落盘。

### 3.2 稳定 id/class 过滤

不稳定的 token 会污染相似度，所以在建指纹时先过滤：

| 规则 | 例子 | 处理 |
|---|---|---|
| 纯数字或字母太少（< 3 个字母） | `123`、`a1`、`h2` | 丢弃 |
| 长数字串（连续 ≥ 4 位） | `comments-12345` | 丢弃 |
| 整串哈希 / 带语义前缀的哈希后缀 | `8f3a2b1c`、`css-8f3a2b1c` | 丢弃 |
| CSS module 后缀 | `postTitle__aB3x` → `posttitle` | 保留语义前缀 |
| 普通语义 token | `post-title`、`story-card__body` | 保留（小写） |

规则刻意保守：宁可少用一个 class，也不把缓存戳当语义。代价是 CSS-in-JS 站点的 class 特征几乎全丢，只能靠文本与路径。

### 3.3 相似度

分数是 6 个分量的加权平均；**只在指纹有该特征时**才把对应权重计入分母（缺失特征不惩罚候选）：

| 分量 | 权重 | 计算 |
|---|---|---|
| tag | 0.15 | 相同得满分，否则 0 |
| 稳定 id | 0.10 | 完全相同得满分 |
| 稳定 class | 0.15 | 两个集合的 Jaccard |
| 自身文本 | 0.35 | 归一化后字符二元组 Sørensen–Dice |
| 邻近文本 | 0.10 | before+after 的字符二元组 Dice |
| 层级路径 | 0.15 | 祖先 tag 链 LCS（0.75）+ 同级索引接近度（0.25；深度不同时为 0） |

文本归一化只保留字母/数字并转小写，标点、空白、货币符号变分隔符。整个打分是确定性计算，没有随机性、没有模型、没有网络。

### 3.4 默认参数

| 参数 | 默认值 | 理由 |
|---|---|---|
| `min_confidence` | 0.60 | 低于它宁可不返回；本 fixture 的正确候选在 0.76–0.97，错误场景最高 0.51 |
| `min_margin` | 0.06 | 前两名太近就判歧义，不二选一 |
| `top_k` | 3 | 让调用方能看见竞争候选 |
| `max_scan_nodes` | 5000 | 大页面硬上限；超出标 LIMIT |
| `max_candidates` | 200 | 控制完整打分数量；超出标 LIMIT |
| `max_elapsed_ms` | 50 | 相似度阶段墙钟上限；超时返回 TIMEOUT |
| `max_text_chars` | 200 | 指纹/候选文本上限 |
| `max_near_chars` | 80 | 邻近文本上限 |

这些是研究默认值，不是产品承诺；实现前必须在真实站点上重新校准。

### 3.5 输出语义

| 情况 | status | matched_by | confidence | requires_confirmation | 信封 |
|---|---|---|---|---|---|
| 当前选择器仍命中 | `exact` | `selector` | 1.0 | false | OK |
| 零命中，唯一候选 ≥ 阈值 | `relocated` | `similarity` | 候选分 | true | OK |
| 零命中，前两名分差 < margin | `ambiguous` | `similarity` | 最高分 | true | OK |
| 零命中，最高分 < 阈值 | `not_found` | `none` | 最高分 | false | OK |
| 指纹特征 < 2 | `insufficient_fingerprint` | `none` | 0.0 | false | OK |
| 扫描/候选截断 | 依上 | 依上 | 依上 | 依上 | LIMIT + degraded_from |
| 超墙钟 | — | — | — | — | TIMEOUT（无 data） |

关键点：**not_found 仍然是 ok=true**，因为"找不到候选"不是解析失败；它与 exact/relocated 的区别全部在 data 里，调用方不需要靠 error 猜。

---

## 4. 离线实验

### 4.1 样本

全部是本地 fixture，测试不联网、不起子进程：

| fixture | 用途 |
|---|---|
| `news_v1.html` / `news_v2.html` | 同一站点改版：div → article、class 重命名、插入 heading 包裹层、条目 2/4 换序 |
| `news_v3_radical.html` | 内容与结构同时大改：标题全换、h2 变 span+a、层级变浅 |
| `duplicate_v1.html` / `duplicate_v2.html` | v2 同时有桌面端和移动端两份相同卡片，制造真实歧义 |
| `thin_v1.html` / `thin_v2.html` | 锚点只有 tag/祖先、没有文本/属性，测试拒绝 |

### 4.2 改版命中率

v1 → v2，6 个锚点，全部要求 status=relocated、matched_by=similarity、requires_confirmation=true：

| 锚点选择器（v1） | 期望文本 | confidence | 结果 |
|---|---|---|---|
| `#post-1 .post-title a` | City opens new riverside park | 0.918 | relocated |
| `#post-2 .post-title a` | Council approves night bus plan | 0.918 | relocated |
| `#post-3 .post-title a` | Library extends weekend hours | 0.918 | relocated |
| `#post-4 .post-title a` | Farmers market moves to the square | 0.918 | relocated |
| `#post-5 .post-title a` | Harbour clean-up draws volunteers | 0.918 | relocated |
| `#post-2 .post-body p` | Night buses will run every thirty minutes | 0.971 | relocated |

**命中率 6/6。** 注意：选择器里的 `#post-N` 在 v2 已不存在，v2 的 id 是 `story-N` 且顺序不同；能命中靠的是文本、路径与邻近特征，不是 id。日期变化的 `.post-meta` 也单独测了：仍能重定位，但置信度不高于文本不变的标题（见测试 ADAPT-04）。

### 4.3 负样本与歧义

| 场景 | 期望 | 实际 |
|---|---|---|
| v1 → v3（内容 + 结构大改） | not_found，不硬塞候选 | 最高 0.508，低于 0.60；not_found |
| 重复 DOM（桌面 + 移动） | ambiguous，前两名平分 | 0.933 / 0.933，margin=0 |
| 只有 tag/祖先的锚点 | insufficient_fingerprint | 只有 1 个有效特征 |
| 扫描上限=1 | LIMIT + degraded_from | scanned=1，code=LIMIT |
| 预算=0 | TIMEOUT，无 data | code=TIMEOUT |

### 4.4 耗时与预算

fixture 上单次调用的实测值（debug 构建，16 核机器；命令见 §7）：

| 场景 | meta.elapsed_ms | scanned_nodes | considered_candidates |
|---|---|---|---|
| 标题重定位（5 个锚点） | 5–6 | 49 | 8 |
| 正文重定位 | 7 | 49 | 11 |
| 重复 DOM（歧义） | 1 | 13 | 2 |
| 扫描上限=1 | 2 | 1 | 0 |

远低于默认 50 ms 预算，但这只是 2–3 KB 的 fixture，**不能**外推到大页面。需要强调的是：**相似度只在精确选择器零命中后才跑**。精确命中路径多解析一次当前文档，不扫描候选，也不受 50 ms 相似度预算约束；这保证"正常页面不变慢"。

### 4.5 实验的局限

- 样本是手写的受控改版，不是真实站点快照；6/6 不能外推成线上命中率。
- 没有做 A/B、没有多语言页面、没有大体量页面（1 MB+）性能测试。
- confidence 是启发式分数，不是校准概率；0.60 这个阈值只在本样本上区分度够用。
- 只测了一个锚点；一个选择器命中多条时只取第一条。

---

## 5. 什么情况下不值得做

以下场景即使技术上"能跑"，也不应自动重定位：

1. **结构大改且文本也变。** 本样本里 v3 的最高分低于阈值，正确结果是 not_found；如果强行降低阈值，只会把无关元素塞给调用方。
2. **重复组件/重复 DOM。** 列表卡片、桌面+移动双份 DOM、同文案的多个入口，前两名往往同分。没有额外上下文（位置、父容器语义、业务 id）时，二选一就是猜。
3. **文本本身是动态的。** 价格、库存、日期、计数、AB 实验文案频繁变化；文本 Dice 会随变化幅度下降，阈值附近的行为不可预测。
4. **锚点没有可比较特征。** 图标、空 div、纯图片链接、只有 tag 的容器，指纹不足，应该直接拒绝。
5. **选择器本来就命中一个集合。** 单锚点模型无法回答"这 20 条里哪条对应上一版哪条"；需要的是集合对齐，不是单元素相似度。
6. **大页面 + 严格延迟预算。** 5000 节点、200 候选是上限，不是免费；对延迟敏感的高频采集，重定位应与正常抽取分开计费/限流。
7. **合规上不能猜。** 法律、医疗、金融等场景需要确定性；相似度结果必须人工/调用方确认，不能作为事实写入。
8. **跨请求状态与隐私。** 保存指纹意味着保存页面文本片段；若要做持久化，需要过期、脱敏、按域隔离的完整设计，本 spike 不支持。

一句话：**只有在"页面小改、锚点有稳定文本/语义、候选唯一"时才值得；其余情况，如实零命中比猜一个更有价值。**

---

## 6. 建议

### 6.1 若实现

1. **显式 opt-in，不默认。** 不新增第 19 个工具（工具面保持 18）；在 `extract` 上增加可选输入（例如调用方回传上一版 `ElementFingerprint` 或开启 `adaptive`），默认关闭。
2. **零命中语义不变。** 没有 opt-in 时，零命中仍是 OK 空结果；有 opt-in 时，相似度结果只进新字段 `adaptive` / `candidates`，不覆盖 `matches`。
3. **标签与证据不可省略。** `matched_by=similarity`、`confidence`、`requires_confirmation=true`、每条候选的 `evidence` 必须同时出现；缺一个就不合协议。
4. **阈值与预算可配置但有硬上限。** 默认沿用本 spike；服务端硬上限防止调用方把 50 ms 调成 5 s。
5. **先补 ADR。** 见 [ADR-0009 草案](adr/0009-adaptive-selectors.md)：记录"相似度结果永远是候选、不自动替换"的边界。
6. **真实站点校准。** 至少在 10 个站点、20 组改版快照上测命中率/误报率，再决定阈值是否发布。

### 6.2 不建议

- 不要让 `extract` 自动"找回"元素后把 `matched_by` 藏起来；
- 不要把相似度候选写进 `matches` 冒充精确命中；
- 不要默认开启；不要让一次重定位拖慢所有请求；
- 不要在没有过期/脱敏设计时持久化指纹。

---

## 7. 复现

~~~bash
export CARGO_TARGET_DIR=/home/thatgfsj/AIagent/webctl-target-195
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_INCREMENTAL=0
export CARGO_BUILD_JOBS=3
export BINDGEN_EXTRA_CLANG_ARGS="-I/usr/lib/gcc/x86_64-linux-gnu/14/include"

cd /tmp/webctl-wt-195
cargo test -p webctl-web --test adaptive -- --nocapture
~~~

测试直接 `include_str!` 读取 fixture，不联网、不写盘、不启动子进程。2026-11-23 本地结果：10 passed / 0 failed，测试耗时 0.03 s。
