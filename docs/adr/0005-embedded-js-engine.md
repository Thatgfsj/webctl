# ADR-0005：引入受限内置 JS 引擎（boa_engine），默认关闭

- 状态：已采纳（2026-10-17）
- 关联：[议题 #118](https://github.com/Thatgfsj/webctl/issues/118)、[EPIC #113](https://github.com/Thatgfsj/webctl/issues/113)、[ADR-0001](0001-single-layer-builtin.md)、**取代 [ADR-0002](0002-no-js-engine-in-v01.md)**

## 背景

[ADR-0002](0002-no-js-engine-in-v01.md) 决定 v0.1 不内置任何 JS 引擎，对 JS 页面一律显式降级。该决策当时的依据之一是"维护者未答复 ⇒ 取最保守且可逆的默认值"，并留下一条缝：**将来引入引擎时，本 ADR 被新 ADR 取代，且必须先给出"内置引擎 vs 继续降级"的成本对比**。

2026-10-17 维护者明确要求引入引擎，理由是真实使用中**JS 密集型 / SPA 站点取不到有效信息**（0.1.2 目标，见 EPIC #113）。本 ADR 履行 ADR-0002 留下的义务：给出实测成本对比后再改决策。

## 决策

**引入一个受限的内置 JS 引擎：`boa_engine`（纯 Rust）。** 边界如下，逐条都是可验收的：

1. **唯一允许**：`Cargo.lock` 中只允许 `boa_engine` 一个 JS 引擎；禁止 `v8` / `deno_core` / `quickjs` / `rquickjs` / `mozjs` / `duktape` / 任何外部浏览器自动化依赖。由 `scripts/check-dependency-boundary.sh` 在 CI 的 `guards` 作业里机械校验。
2. **默认关闭**：不改变默认行为——**默认仍不执行页面 JS**。开启是显式的（配置或逐调用参数），且 `caps` 会报告可用性与当前状态。这样 ADR-0002 立下的降级锚点**继续成立**（例如 `quotes.toscrape.com/js/` 在默认配置下仍必须 `DEGRADED`）。
3. **硬限制，跑不飞**：指令数 / 循环迭代数 / 墙钟 / 内存都有上限；**引擎内不暴露网络与文件系统**（不注入 `fetch`/`XMLHttpRequest`/`fs`）；超限必须返回 `TIMEOUT` / `LIMIT` 信封，**禁止静默降级**（A3）。
4. **自建最小 DOM/BOM**：只实现取内容需要的子集（`querySelector(All)`、`textContent`、`getAttribute`、只读 `innerHTML` 等）；**实现不了的必须如实 `UNSUPPORTED`**，不做"半个浏览器"。
5. **零外部进程、零外部浏览器不变**（ADR-0001 有效）：引擎是**进程内库**，不启动任何子进程。
6. **反爬姿态不变**（ADR-0004 有效）：内置引擎**不是**真实浏览器指纹，不承诺绕过 Cloudflare / 验证码，遇到即 `BLOCKED`。

## 备选方案（含**实测**成本对比）

| 备选 | 实测/评估 | 结论 |
|---|---|---|
| **boa_engine 0.20（纯 Rust）** | 依赖 **+130 包**；最小静态 musl 二进制 **11,801,480 B**（glibc 12,063,240 B）⇒ 相对"零引擎"约 **+11 MB**；**musl 静态交叉不需要任何 C 工具链**（实测 `statically linked`，1m19s 构建）；**硬限可用**：`loop_iteration_limit` 让死循环在 **177 ms** 被中断并报 `RuntimeLimit` | ✅ **采用** |
| rquickjs（QuickJS + C） | 更快更小，但引入 C 交叉工具链；本仓已被 BoringSSL + musl 的 C++ 工具链咬过三次（[#90](https://github.com/Thatgfsj/webctl/issues/90)） | ✗ 不在本期 |
| 继续降级（ADR-0002 原方案） | 零成本、零风险，但**不解决用户反馈的问题** | ✗ |
| 外部 headless 浏览器 | 冷启动 292–313 ms、协议漂移、进程泄漏，且与 ADR-0001 直接冲突 | ✗ |
| V8 / deno_core | 体积与构建复杂度量级更高，收益对"取文本"这个目标过剩 | ✗ |

## 后果

- **体积基线要重定**：静态资产预计从 22.6 MB 增到 ~33–34 MB（+~50%）。L2 的 `scripts/size-baseline.sh --check` 必须用**真实发布资产**重新回填，并在 CHANGELOG 里写明增长归因（引擎本身）。
- **默认行为不变**：不开启时，一切既有验收锚点（L1 的 `DEGRADED` 断言、L0/L3 全部用例）**不受影响**；这是本决策刻意保留的可逆性。
- **新增安全面（关键）**：把**不可信页面的 JS** 交给进程内解释器执行，沙箱与限制从"没有"变成"安全关键项"。因此引擎接线 PR 必须**独立安全审阅**，并包含：无网络/无文件的能力确认、指令与时间上限、内存上限、逃逸与 DoS 用例。
- **能力边界的诚实预期**：boa 是解释器，**不等于浏览器**——没有布局/渲染引擎，复杂现代框架（React/Vue 水合、WebGL、大量 DOM API）大概率跑不动。它能覆盖的是"内容由中等规模 JS 生成"的页面。真实覆盖率由 #119 的基线给数字，**不做承诺**。
- ADR-0002 被本 ADR 取代；其正文不改写，仅在状态处标注。

## 证据

spike（2026-10-17，本机 `/tmp/js-spike`，`CARGO_BUILD_JOBS=2`、nice 10、target 在 `~/.cache`）：

~~~text
# 依赖
Locking 130 packages to latest compatible versions
  Adding boa_engine v0.20.0 (available: v0.22.0)

# host release
  Finished `release` profile [optimized] target(s) in 1m 37s
  host 二进制字节: 12063240

# 运行：基本求值 / 硬限 / 吞吐
basic = "{"a":[1,2,3],"b":"x"}"
loop_limited = Some("RuntimeLimit: Maximum loop iteration limit 1000000 exceeded") in 177.742974ms
throughput = Ok("857780") in 1.506012942s

# musl 静态交叉（无 C 工具链）
  Finished `release` profile [optimized] target(s) in 1m 19s
  musl 二进制字节: 11801480
  ELF 64-bit LSB pie executable … statically linked
~~~

> 说明：spike 是"最小可运行程序 + 引擎"，用于测量**体积增量与可行性**；它不是 webctl 的接线实现。接入后的真实体积以 L2 基线（真实发布资产）为准。
