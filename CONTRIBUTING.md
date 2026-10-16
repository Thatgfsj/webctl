# 贡献指南

本项目的流程不是可选项：**议题先于代码**。开工前请读完本文件与 [docs/PLAN.md](docs/PLAN.md)。

## 1. 开工前必须做的事

| 规则 | 说明 |
|---|---|
| **feat 前先写 issue** | 新功能、新 crate、协议扩展、依赖变更都必须先有 issue，并在 issue 中说明目标、写入范围与验收标准 |
| **PR 必须关联 issue，且先领取** | 在 issue 下留言认领（例如 `/claim`），确认无人重复后再动手；未关联 issue 或无认领的 PR 不通过 |
| **一个 PR 只做一类变更** | 禁止"顺手规范化"：协议类型、传输、会话语义、工具面、挂载/治理不得混改 |
| **修复落在引入它的那个 PR** | 缺陷修复开在引入缺陷的 PR 上，不新建"清理 PR" |
| **≥2 层依赖链用官方 stack** | 需要堆叠 PR 时使用 GitHub 官方 stacked PR；部分落地须显式开边界 PR |

## 2. PR 行数口径（≤1000 行）

> 单个 PR 的 `git diff --shortstat` 改动行数（additions + deletions）目标不超过 1000 行；超过时需维护者在关联 issue 中显式批准并写明理由。生成产物、协议类型批量录入、纯文档与测试 fixture 不计入该口径，但必须在 PR 描述中列明实际行数。

机械口径：

- **计入** = `git diff --shortstat <base>...HEAD` 的 `additions + deletions`。
- **不计入**：`docs/` 纯文档、测试 fixture 数据文件。（生成产物如 `Cargo.lock` 的归属见议题 #41，尚未定案。）
- **不计入 ≠ 不申报**：PR 描述必须写明这些文件的实际行数，避免其成为后门。
- 越线需在关联 issue 中显式批准；批准记录随 PR 保留。

两点已知事实（见 [docs/SURVEY.md §1.2](docs/SURVEY.md)）：

1. 本规则是**新写入的项目规则**，此前在 T-Auto / Thatgfsj 任何仓库中都没有成文依据；不要引用不存在的上游条款。
2. 组织级真实门禁是**加权 2 分制**（`T-Auto/.github` 的 `approval-policy.json`），由行数无关的评审权重决定；要让它与行数联动，只能改 CI，不能只写文档。

## 3. 目录与模块边界

```
crates/
├─ webctl/        # bin：CLI 入口 + MCP 入口（feature 切换）
├─ webctl-core/   # envelope / 错误码 / 配置 / 能力自检 / 会话状态 / 安全脱敏
├─ webctl-web/    # 取回 + 解析 + 会话 + 文档与数据（全部内置）
└─ webctl-mcp/    # rmcp server + 工具 schema + SKILL.md 生成
```

- 依赖方向单向：`bin → mcp → core`、`bin → web → core`，`core` 不依赖任何上层。
- **运行时零外部进程、零浏览器**：代码里不得出现子进程调用（`std::process::Command` 等），也不得引入 CDP/WebDriver 依赖。
- **不要为了"看起来模块化"新增 crate**：只有 (a) 变更来源或节奏明显不同、且 (b) 依赖方向单向无环时，才拆成独立 crate。
- 每个 crate 稳定后自带一份 `CONTRIBUTING.md`，声明本模块的边界与验收（照搬 biome / ruff 的组织方式）。

## 4. 门禁命令

| 阶段 | 命令 | 通过标准 |
|---|---|---|
| 每 PR | `cargo fmt --check && cargo clippy -- -D warnings && cargo test` | 全绿 |
| L0 离线 lane | `cargo test --features offline`（该 feature 由议题 #22 落地；此前离线性由各测试自身的 `127.0.0.1` fixture 保证） | 取回/解析/会话/文档用例全绿（不依赖外网） |
| 真站 smoke | `./tests/e2e/run-real.sh` | 静态锚点取到 10 条；JS 页返回 `DEGRADED` |
| 体积基线 | `cargo build --release --target x86_64-unknown-linux-musl` | 不超 P0 基线，防回归 |
| PR 行数 | `git diff --shortstat` 对照 §2 | ≤1000 或已获批准 |

> **v0.4 起没有"无浏览器 lane"和"mock-cdp lane"**：运行时不再有浏览器，所有用例本来就无浏览器、且不得依赖外部进程。

> **本地构建前置**：取回层依赖 `wreq` → BoringSSL，编译期需要 `cmake` + C/C++ + **libclang**（bindgen）。缺它时报 `Unable to find libclang` 或 `'stddef.h' file not found`：装 `libclang-dev` / `clang`，或按 [docs/spikes/wreq.md §4](docs/spikes/wreq.md) 设置 `LIBCLANG_PATH` 与 `BINDGEN_EXTRA_CLANG_ARGS`。**运行时不依赖这些**；CI 三平台镜像自带。


## 5. PR 描述必须包含

1. 关联 issue（`Closes #N` / `Refs #N`）与认领记录。
2. `git diff --shortstat` 结果，以及不计入口径文件的实际行数。
3. 变更类别（本 PR 只做哪一类）。
4. 已跑的门禁命令与结果；未跑的要写明原因。
5. 涉及降级语义时，说明 `meta.engine` / `code` / `meta.degraded_from` 的表现。

模板见 [.github/PULL_REQUEST_TEMPLATE.md](.github/PULL_REQUEST_TEMPLATE.md)。
