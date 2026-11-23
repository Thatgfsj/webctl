# 抽取质量基准：段落级 F1 锚点（议题 #189）

**为什么有这个文件**：仓库此前只有 SPA「有效信息获取率」基线（[SPA-BASELINE](SPA-BASELINE.md)），
正文抽取质量没有可回归的分数——只有"感觉还行"。本基准给 `parse::article` 一把离线、
可复现的段落级尺子；它是**开发工具**，不在默认 CI 矩阵里，也不改产品默认行为。

> 结论先说（2026-11-23 实测，10 个离线样本）：webctl `markdown` 视图
> **P 0.806 / R 0.952 / F1 0.873**；同批样本、同一把尺子，trafilatura 2.3.1
> **P 0.902 / R 0.892 / F1 0.897**；webctl `text` 视图 F1 0.863。
> 这不是"接近 SOTA"：样本只有 10 个、且是本仓自建的，与 rs-trafilatura 报告的
> 0.966（ScrapingHub 数据集）**不可直接比较**（见 §6）。

## 1. 指标口径

| 项 | 定义 |
|---|---|
| 段落 | **一行一段**：非空行即一个段落（参考文本、webctl 的两个视图、外部工具 txt 都按此切分） |
| token | 拉丁字母/数字连续串（转小写）为一个 token；CJK（汉字/假名/谚文）逐字；其余字符是分隔符 |
| 过滤 | 每侧段落 < `--min-tokens`（默认 4）不计；标题行不计（见下） |
| 配对 | 段落间多重集 **Dice** 系数；Dice ≥ `--threshold`（默认 0.6）才可配对；按 Dice 降序贪心**一对一**配对（`total_cmp` + 下标次序，确定性，无随机种子） |
| P / R / F1 | `P = 配对预测段 / 预测段总数`，`R = 配对参考段 / 参考段总数`，F1 为调和平均 |
| 标题 | 标题是元数据，**不计分**。webctl 把标题放在 `Article.title`；trafilatura 的 txt 把标题放首行；评分前若首行 token 与 `<title>`/`<h1>` token 互为子集，整行剔除 |
| 打分视图 | 默认打 `Article.markdown`（结构保真）；`--view text` 打 html2text 视图（见 §4.1 实测差） |

**语料约定**（写 fixtures 时必须遵守）：

- 参考段落 = 人类读者眼里的正文：正文段、列表项、代码行、引用块、小节标题、日期行；
- **不计**：byline、面包屑、楼层元信息、签名档、博客评论区、食谱用户评论、信息框、
  参考文献、相关阅读、广告、页脚、订阅框；
- 论坛页的**回复正文计入**（论坛的"正文"就是整条线程），博客页的**评论不计**（讨论不是正文）。
  这条约定会影响外部工具的分数，必须连同 §3 的命令一起读；
- fixtures 的文本全部为本仓自写，不含第三方受版权保护内容。

## 2. 本地样本（10 个，入库）

位置：`crates/webctl-web/tests/fixtures/extraction_bench/`；清单：`manifest.json`（顺序即报告顺序）。

| 样本 | 类别 | 页面特征 | 参考段 |
|---|---|---|---|
| `news-001` | news | 新闻正文：导航、广告、相关阅读、订阅框、页脚 | 5 |
| `blog-002` | blog | 个人博客：侧栏、三条长评论（不计）、标签、订阅表单 | 5 |
| `docs-003` | docs | 文档页：侧栏目录、两段代码块（逐行计段）、提示框 | 10 |
| `docs-004` | docs | 更新日志：版本小节 + 项目符号列表 | 11 |
| `forum-005` | forum | 论坛帖：四个楼层正文计入，签名档/楼层元信息/相关帖不计 | 6 |
| `forum-006` | forum | 问答页：提问 + 采纳回答 + 两个其他回答 | 7 |
| `list-007` | list | 清单型页面：分组标题 + 12 条条目 + 贡献说明 | 15 |
| `wiki-008` | wiki | 百科条目：信息框与参考文献不计，正文六段 + 小节标题 | 7 |
| `recipe-009` | recipe | 食谱页：配料与步骤逐条计段，用户评论与广告不计 | 13 |
| `zh-010` | zh | 中文新闻：验证 CJK 逐字分词与中文页面抽取 | 4 |

合计 **83 个参考段**。fixtures 约 42 KB / 688 行（不计入 PR 行数，申报即可）。

## 3. 复现命令

```bash
# 每个 worktree 用独立 target，避免互相踩（本任务示例）
export CARGO_TARGET_DIR=/home/thatgfsj/AIagent/webctl-target-189
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=3
export BINDGEN_EXTRA_CLANG_ARGS="-I/usr/lib/gcc/x86_64-linux-gnu/14/include"

# 一键：webctl(markdown) + webctl(text) + 可选 trafilatura 对照；报告落 /tmp
scripts/extraction-benchmark.sh

# 只跑 webctl；或换报告目录/构建 profile
scripts/extraction-benchmark.sh --no-external
REPORT_DIR=/tmp/bench BENCH_PROFILE=dev scripts/extraction-benchmark.sh

# 直接跑 example（自定义 fixtures / 阈值 / 单样本核对）
cargo run --release --example extraction_bench -- \
  --report /tmp/webctl-extraction-bench/webctl.json
cargo run --release --example extraction_bench -- --view text \
  --report /tmp/webctl-extraction-bench/webctl-text.json
cargo run --release --example extraction_bench -- --dump forum-006
cargo run --release --example extraction_bench -- \
  --fixtures /tmp/my-fixtures --threshold 0.5 --min-tokens 3

# 外部对照（同一把尺子）：<id>.txt 每个样本一个文件
cargo run --release --example extraction_bench -- \
  --predictions /tmp/webctl-extraction-bench/predictions-trafilatura \
  --report /tmp/webctl-extraction-bench/trafilatura.json
```

trafilatura 对照（可选；未安装时脚本打印"跳过"，不伪造数字）：

```bash
python3 -m venv /tmp/traf-venv && /tmp/traf-venv/bin/pip install trafilatura==2.3.1
TRAFILATURA_PYTHON=/tmp/traf-venv/bin/python scripts/extraction-benchmark.sh
```

外部工具调用口径：`trafilatura.extract(html, output_format="txt")`，其余参数默认
（含 `include_comments=True`、`include_tables=True`）；脚本把结果写到
`$OUT/predictions-trafilatura/<id>.txt`，再用同一把尺子打分。
**安装 trafilatura 需要网络，基准本身不联网**。

报告字段：`schema / metric / source / view / settings / totals / categories / samples`；
`samples[]` 带 `missed` 与 `spurious`（截断到 160 字符），失败样例可直接定位。
默认报告路径（仓库外，不入库）：`/tmp/webctl-extraction-bench/webctl.json`、
`webctl-text.json`、`trafilatura.json`。

## 4. 实测结果

时间 **2026-11-23**；代码 `main` @ `8e8fbb1` + 本任务未提交改动；trafilatura **2.3.1**（Python 3.13）。
`dev` 与 `release` 两次运行数字一致（抽取结果与 profile 无关；本基准不测性能）。

### 4.1 总体（micro，83 个参考段）

| 系统 | P | R | F1 | 预测段 |
|---|---|---|---|---|
| webctl `parse::article`（markdown） | 0.806 | 0.952 | **0.873** | 98 |
| webctl `parse::article`（text） | 0.790 | 0.952 | 0.863 | 100 |
| trafilatura 2.3.1（txt，默认参数） | 0.902 | 0.892 | **0.897** | 82 |

### 4.2 逐样本

| 样本 | 类别 | 参考段 | webctl P | webctl R | webctl F1 | trafilatura P | trafilatura R | trafilatura F1 |
|---|---|---|---|---|---|---|---|---|
| news-001 | news | 5 | 0.833 | 1.000 | 0.909 | 1.000 | 1.000 | 1.000 |
| blog-002 | blog | 5 | 0.833 | 1.000 | 0.909 | 0.625 | 1.000 | 0.769 |
| docs-003 | docs | 10 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 |
| docs-004 | docs | 11 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 |
| forum-005 | forum | 6 | 0.600 | 1.000 | 0.750 | 1.000 | 0.333 | 0.500 |
| forum-006 | forum | 7 | 1.000 | 0.429 | 0.600 | 1.000 | 0.286 | 0.444 |
| list-007 | list | 15 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 | 1.000 |
| wiki-008 | wiki | 7 | 0.583 | 1.000 | 0.737 | 0.778 | 1.000 | 0.875 |
| recipe-009 | recipe | 13 | 0.765 | 1.000 | 0.867 | 0.812 | 1.000 | 0.897 |
| zh-010 | zh | 4 | 0.500 | 1.000 | 0.667 | 1.000 | 1.000 | 1.000 |

### 4.3 按类别（micro）

| 类别 | 样本 | webctl F1 | trafilatura F1 |
|---|---|---|---|
| news | 1 | 0.909 | 1.000 |
| blog | 1 | 0.909 | 0.769 |
| docs | 2 | 1.000 | 1.000 |
| forum | 2 | 0.692 | 0.471 |
| list | 1 | 1.000 | 1.000 |
| wiki | 1 | 0.737 | 0.875 |
| recipe | 1 | 0.867 | 0.897 |
| zh | 1 | 0.667 | 1.000 |

## 5. 差距与失败样例（如实）

- 总体：trafilatura 领先 **0.024 F1**；webctl 的 R 更高（0.952 vs 0.892），P 更低（0.806 vs 0.902）。
- webctl 领先的类别：**forum**（0.692 vs 0.471）、**blog**（0.909 vs 0.769）。
- trafilatura 领先：news（0.909 vs 1.000）、wiki（0.737 vs 0.875）、recipe（0.867 vs 0.897）、
  zh（0.667 vs 1.000）；docs 与 list 打平（都是 1.000）。
- 差距的实质（按页面类型）：

| 类型 | 观察到的行为 |
|---|---|
| 论坛型 | webctl 把整条线程抽出来（forum-005 R=1.000），代价是面包屑、楼层元信息、签名档成为误报（P=0.600）；trafilatura 只抽首帖（forum-005/006 R=0.333/0.286），precision 高但召回很低。**问答页两者都不可靠**：forum-006 中 webctl 只保留"采纳回答"，提问与另外两个回答丢失（R=0.429） |
| 文档型 | 两者都 1.000；两个代码块按行计段也全部命中 |
| 列表型 | 两者都 1.000（15/15）；按行切分后，列表项不再被"合并成一段"误判 |
| 新闻/博客 | webctl 的误报来自文章内部组件：新闻页的**订阅框**（"Get the Ledger's morning briefing in your inbox."）、博客页的**作者行**（"Posted on March 3 by Sam Rivera"）；trafilatura 在博客页把**评论区整体抽出**（P=0.625），新闻页干净 |
| 百科/食谱/中文 | webctl 抽出**信息框表格**（`| Location | Built | ...`）、**参考文献列表**、**食谱用户评论**、**懒加载图片 alt**、**中文"相关报道"链接组**；trafilatura 除百科信息框外基本能滤掉。中文样本两者都拿到 4/4 正文段，webctl 的 0.667 全扣在相关阅读的 4 行上 |

- **不要过度解读**：0.024 的总体差在 10 个自建样本上不是统计结论；它说明的是
  "结构与噪声处理的类别差异"，不是"离 SOTA 还有多远"。
- `text` 视图与 `markdown` 视图的差（0.863 vs 0.873）集中在 forum（0.667 vs 0.692）
  与 wiki（0.700 vs 0.737）：html2text 会把个别块边界（如代码围栏、列表前引号）拆/合，
  导致段落数与参考错位；正文内容本身没有丢。

## 6. 与 trafilatura / rs-trafilatura 公开数字的关系

- rs-trafilatura 报 **F1 0.966**（ScrapingHub `article-extraction-benchmark`）；trafilatura 的
  论文（Bevendorff 2023）用的是另一套数据集与指标（段落/词级 LCS 对齐、数百页真实网页）。
- 本基准的 10 个样本是**本仓自写**的合成页面，指标是 Dice 配对；两者**不可比**。
  本文件的数字只用于：(a) 回归锚点；(b) 同一批样本上的 A/B 对照。
- 若将来要跑大数据集：运行期下载、不入库（见 §8），并把数据集名/版本/日期写进报告旁边。

## 7. 局限与不测什么

- 样本 10 个、自建、英文为主（1 个中文样本）；作者偏好会渗进参考集。
- 阈值敏感：`--threshold 0.6` 与 `--min-tokens 4` 是默认口径；换阈值必须重跑并注明。
- 只测 HTML 正文抽取；不测反爬、JS 渲染、PDF、编码、表格/链接等其它通道。
- 评论区约定（博客不计、论坛计）对 trafilatura 不利（它默认 `include_comments=True`）；
  这是"正文"定义的必然结果，不是调参结论。
- 不进默认 CI：每次 PR 跑基准会与实现抢 CPU，也会把样本偏差固化成门禁；发布前或抽取行为
  变更时手动跑。
- 仓库只放脚本 + 10 个 fixture，不放大数据集。

## 8. 维护规则

1. 改 `parse::article` 行为（Readability 配置、噪声列表、回退路径）→ 必须重跑本基准，
   在 §4 更新数字、日期与 commit；**不允许**为了让数字好看而改参考集。
2. 新增样本：加到 fixtures + `manifest.json`，遵守 §1 的语料约定，保持离线、自写、无版权风险。
3. 大数据集（如 article-extraction-benchmark）只在运行期下载到 `/tmp`，报告里写清来源与许可；
   仓库不提交。
4. 本基准不是门禁；要把它变成门禁需单独立项（样本量、抖动、平台差异都还没解决）。
5. 工具面仍是 18 个工具：本基准是 `examples/` + `scripts/` 的 dev 工具，不注册 MCP 工具、
   不改产品默认行为。
