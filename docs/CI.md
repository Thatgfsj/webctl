# CI 说明

> 一句话：把本地能跑的每条门禁**在三平台各跑一遍**，并且任何失败都要**指名道姓**（`文件:行:列`），
> 而不是让人在几千行日志里翻。文件名 `.github/workflows/ci.yml`；本文说明它为什么这么切、怎么读结果、以及边界在哪。

## 1. 作业一览

| 作业名（GitHub 上就是这个字符串） | 平台 | 覆盖什么 | 超时 | 为什么单独一个作业 |
|---|---|---|---|---|
| `static · fmt / scripts / config` | ubuntu | 格式、`scripts/*.sh` 语法、**报错定位器自检**、cargo-dist 生成物一致性 | 20 min | 最便宜的错误最先挡住；不需要编译 |
| `clippy (ubuntu-latest / macos / windows)` | 三平台 | `clippy --all-targets -D warnings`（默认 + offline 两条通道） | 40 min | 不同平台的 cfg / 依赖解析差异只有矩阵能暴露 |
| `test (ubuntu-latest / macos / windows)` | 三平台 | **默认门禁 + L0 离线通道 + release 构建**；忽略测试零容忍；工作区洁净 | 60 min | 主体（工具/套件测试都在这里） |
| `guards · 锁文件 / 工具面 / 行数口径` | ubuntu | `cargo metadata --locked`、SKILL.md ↔ tools/list 一致、PR 行数口径 | 20 min | 与"能不能编译"正交的合并安全项 |
| `musl-static (x86_64-unknown-linux-musl)` | ubuntu | Bootlin 工具链构建 + `file`/`ldd`/`readelf` 静态断言 | 60 min | 议题 [#90](https://github.com/Thatgfsj/webctl/issues/90) |

## 2. 通道映射（对应 [TESTING.md §1](TESTING.md) 的 L0–L4）

| 通道 | CI 里怎么跑 | 本地等效命令 |
|---|---|---|
| **L0 离线** | `test` 作业第 2 条通道：`cargo test --locked --features offline` | 同左 |
| **L1 真站 smoke** | **不在 CI 跑**（要外网、易抖动） | `./tests/e2e/run-real.sh` |
| **L2 体积 / 静态** | `test` 作业的 `cargo build --release --locked` + `musl-static` 作业 | `scripts/size-baseline.sh --check` |
| **L3 安全负面** | 并入默认门禁（在**默认配置**下跑，不加任何放宽开关） | 同 `cargo test` |
| **L4 性能基线** | **不在 CI 跑**（机器噪声大，手动通道） | `scripts/perf-baseline.sh --check` |

## 3. 报错是怎么被"精准"定位的

每个可能失败的命令都把输出 `tee` 到 `$RUNNER_TEMP/*.log`（不落进仓库，避免污染工作区），随后 `scripts/ci-report.sh` 做三件事：

1. **注解**：`::error file=<仓库相对路径>,line=<行>,col=<列>,title=<通道 · 类型>::<消息>` —— 直接出现在 PR 的 Files changed 视图与 Checks 页；
2. **作业摘要**：往 `$GITHUB_STEP_SUMMARY` 写一张表（passed / failed / ignored / 注解数 + 失败位置清单）；
3. **原始日志**：作为 artifact 上传（成功也传，失败时更好用）。

它解析三类信号：

- 编译 / clippy：`error[Exxxx]: msg` + 紧随的 `--> path:line:col`；
- 测试失败：`test NAME ... FAILED` + `thread ... panicked at path:line:col:` + **下一行的 panic 文案**；
- 构建失败汇总：`error: could not compile ...`。

**它的可靠性由自检保证**：`scripts/ci-report.sh --selftest` 用**植入式夹具**断言 9 条（**以该命令的输出为准**，条数会随哨兵增加而变），包括：Windows 路径（`D:\a\webctl\webctl\...`）必须规范成仓库相对路径、干净日志**零误报**、把夹具行号从 791 改成 999 后定位必须跟着变（证明不是硬编码）。这一步是 `static` 作业的一个步骤——**报错机制本身也被 CI 测试**。

## 4. 合并安全（为什么"绿"值得信）

- **所有 cargo 命令带 `--locked`**：`Cargo.lock` 与清单不一致直接红，不允许锁文件悄悄漂移；
- **`concurrency` 取消同 ref 的旧运行**：避免"旧的绿掩盖新的红"；
- **最小权限**：workflow 级 `contents: read`，只有 `guards` 额外要 `pull-requests: read`；
- **每个作业都有 `timeout-minutes`**：挂死不会占满 runner；
- **忽略测试零容忍**：出现 `#[ignore]` 即红，并逐条给出 `file:line` 注解；
- **测试后工作区必须洁净**：与**跑测试前记录的状态做对比**，只判"测试过程新增"的未跟踪/被修改文件（不要求绝对为空——否则 Windows 上可能因换行符设置产生假脏）；
- **生成物不许漂移**：触及 `dist-workspace.toml` / `.github/build-setup.yml` / `.github/workflows/release.yml` 时跑 `cargo dist generate --mode ci --check`；
- **分支保护**：`main` 上把下面这些检查设为必需（见 §5）。

## 5. 必需检查清单（分支保护）

要在 `main` 上强制的检查名（**必须与作业名逐字一致**，否则会把所有合并卡死）：

~~~text
static · fmt / scripts / config
clippy (ubuntu-latest)      clippy (macos-latest)      clippy (windows-latest)
test (ubuntu-latest)        test (macos-latest)        test (windows-latest)
guards · 锁文件 / 工具面 / 行数口径
musl-static (x86_64-unknown-linux-musl)
~~~

> `pr-size` 已并入 `guards`（只在 PR 上跑那一项）。要不要把 `enforce_admins` 也打开是取舍：打开后连管理员也必须走 PR + 绿检查，**紧急直推 main 会被挡住**；本仓当前选择"检查强制、管理员不豁免"，理由见 `CONTRIBUTING.md` 的评审纪律。

## 6. 本地怎么复现 CI

~~~bash
# 与 static 作业等价
cargo fmt --all --check
for f in scripts/*.sh; do bash -n "$f"; done
scripts/ci-report.sh --selftest

# 与 clippy / test 作业等价（--locked 要保持一致）
cargo clippy --all-targets --locked -- -D warnings
cargo clippy --all-targets --locked --features offline -- -D warnings
cargo test --locked --no-fail-fast
cargo test --locked --features offline --no-fail-fast
cargo build --release --locked

# 与 guards 作业等价
cargo metadata --locked --format-version 1 > /dev/null
cargo build --locked -p webctl && scripts/check-skill-tools.sh
~~~

构建前置（libclang / BoringSSL）见 [CONTRIBUTING.md §4](CONTRIBUTING.md) 的说明。

## 7. 已知边界（诚实登记）

- **L1 真站与 L4 性能不在 CI**：前者要外网且站点会抖，后者对机器噪声敏感。所以"**CI 绿 ≠ 真站也绿**"，这两条是本地手动通道；
- **矩阵只覆盖 x86_64**：ubuntu / macos / windows 的 runner 都是 x86_64（macos-latest 是 arm64 还是 x86_64 取决于 GitHub 的轮换），**不覆盖 aarch64 Linux**——那属于发布矩阵问题（见 [RELEASE.md](RELEASE.md) 的"未纳入的目标"）；
- **定位依赖输出格式**：若 cargo / rustc 未来改了 `-->` 或 `panicked at` 的格式，注解会退化为"只能看原文日志"。§3 的自检夹具就是这条退化的哨兵；
- **`cargo-dist` 一致性检查**按需触发（未触及相关文件时跳过并打 notice），所以它**不是**每次都跑。
