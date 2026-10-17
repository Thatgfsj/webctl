# ADR-0001：单层内置——不引入外部进程与外部浏览器

- 状态：已采纳（2026-10-16）
- 关联：[议题 #31](https://github.com/Thatgfsj/webctl/issues/31)、PR #32、[PLAN §0](https://github.com/Thatgfsj/webctl/blob/main/docs/PLAN.md)

## 背景

v0.3 的设计是四层能力阶梯：T0 内置解析、T1 协议级会话引擎、T2 系统 CLI 白名单桥（pdftotext / tesseract / pandoc / jq）、T3 自研 CDP 驱动 Edge/Chrome。调查（SURVEY §2.2/§2.3）证明自研 CDP 在技术上可行：手写帧驱动 Edge、真实点击、中文输入、a11y 树、截图都已实测通过。

但把"能内置的都内置"之后，外部层带来的复杂度不再划算：

- 外部浏览器：协议随 Edge/Chrome 版本漂移；浏览器进程与会话泄漏；冷启动实测 292–313 ms；跨平台可执行文件探测；封禁与反检测是持续博弈。
- 外部 CLI：多一层进程边界与安全面（白名单、路径 canonicalize、注入防护）；本机连 tesseract / pandoc 都没有，"有则启用"意味着能力矩阵随机器而变。

## 决策

**webctl v0.4 只保留单层：所有能力在进程内实现，运行时不启动任何外部进程、不依赖任何浏览器。** 能力按域划分（取回 / 解析 / 会话 / 文档与数据 / 接口），不再有 T0–T3 分层。

## 备选方案

| 备选 | 为什么不选 |
|---|---|
| 保留 T3 真浏览器（自研 CDP） | 复杂度与维护面最大的一块，而主要收益（JS 渲染）在多数 agent 取数任务里不是必需；先做窄而快的内置路径 |
| 保留 T2 外部 CLI 桥 | 能力随机器浮动；#ADR-0002 已决定不做 OCR，而 pdftotext/pandoc/jq 的进程内替代（PR 07）覆盖了主要场景 |
| 仅保留 T2、砍掉 T3 | 保留的还是外部进程边界，收益最小、代价照旧 |

## 后果

**得到**：分发为单个二进制；无浏览器依赖；无进程边界；能力矩阵不随机器的 PATH 变化；冷启动数量级更快；测试可以完全离线。

**失去（必须一直写在 PLAN §0，不许悄悄消失）**：

1. 不执行 JS ⇒ JS 渲染页面拿不到（`quotes.toscrape.com/js/` 仍是 0 条），必须显式 `DEGRADED`/`UNSUPPORTED`。
2. 没有渲染器就没有像素与布局 ⇒ 无截图、无页面 PDF 打印、无按坐标交互。
3. 无成熟纯 Rust OCR ⇒ 扫描件 `UNSUPPORTED`。
4. 反爬姿态变弱 ⇒ 只剩 TLS 指纹与合规请求头/限速，遭遇 Cloudflare 即 `BLOCKED`，不承诺绕过。

**可逆性**：将来若要 JS 渲染，走**可选内置引擎**（Boa 或 rquickjs + 自建最小 DOM），仍不引入外部进程与浏览器。按 capability-seam 纪律，**现在不为它预先拆包**（见 [ADR-0002](0002-no-js-engine-in-v01.md)）。

## 证据

- [SURVEY §2.2 / §2.3](https://github.com/Thatgfsj/webctl/blob/main/docs/SURVEY.md)：手写 CDP 可行性与冷启动时延实测。
- [SURVEY §2.1](https://github.com/Thatgfsj/webctl/blob/main/docs/SURVEY.md)：本机 CLI 可用性（tesseract / pandoc 缺失）。
- [PLAN §0 方向变更与代价清单](https://github.com/Thatgfsj/webctl/blob/main/docs/PLAN.md)。
- `probes/cdp/`：v0.3 时代的探针脚本，保留为可复查的历史证据。

> **现状注记（2026-10-17）**：本条「静态」一项在 Linux 上**尚未成立** —— BoringSSL(btls-sys) 的 C++ 源需要面向 musl 的 libstdc++，而 Debian/Ubuntu 的 `musl-dev` 只提供 C 侧；三次真实 release 均失败（见 [#90](https://github.com/Thatgfsj/webctl/issues/90)）。
> 当前 Linux 发布物为 glibc 动态链接；决策本身（单层内置、无进程边界）未变，仅分发包形态受限。
