# webctl 发布流程（RELEASE）

| 项 | 值 |
|---|---|
| 适用 | **v0.2.0（当前 release**，2026-10-25）、v0.1.2（上一版，2026-10-20）与 v0.1.1 |
| 流水线 | cargo-dist 0.33.0：`dist-workspace.toml` + `.github/workflows/release.yml` |
| 状态 | **v0.2.0 已发布**（2026-10-25；5 目标 / 10 压缩包 / 28 资产；下载 + `sha256sum -c` + `--version`=0.2.0 + musl 静态 + `release-e2e.sh` + Alpine 用户态全部实测，见 ACCEPTANCE §7）；v0.1.1 与 v0.1.2 均已发布。安装命令与资产数字取自 **v0.2.0 真实资产**。 |
| 相关 | [TESTING §2.16 PKG](TESTING.md) · [PLAN §5.2 PR 14](PLAN.md) · [体积基线](size-baseline.json) |

## 1. 流水线结构

- `dist-workspace.toml`：cargo-dist 配置，**唯一事实来源**。`release.yml` 由 `dist generate` 依据它生成；
  `dist generate --check` 可机械确认二者一致（本 PR 已通过，退出码 0）。
- `.github/workflows/release.yml`：cargo-dist 自动生成。触发条件为 push tag（形如 `**[0-9]+.[0-9]+.[0-9]+*`）或 pull request
  （PR 上只跑 `dist plan`，不发布、不建 release）。
- `.github/workflows/ci.yml`：**push main 上**三平台门禁 + `musl-static` 作业（固定 Bootlin musl C++ 工具链，构建并断言静态）；**PR 只跑 `static` / `guards`**（2026-10-24 政策，见 docs/CI.md §8）。

目标平台（`dist-workspace.toml` 的 `targets`）：

| target | 说明 |
|---|---|
| `x86_64-unknown-linux-musl` | Linux **静态**（v0.1.1 起随发布产出）：CI 在 push main 时用固定 Bootlin 工具链构建并断言静态（PR 阶段不跑，见 docs/CI.md §8） |
| `x86_64-unknown-linux-gnu` | Linux glibc 回退 |
| `aarch64-apple-darwin` / `x86_64-apple-darwin` | macOS（Apple Silicon / Intel） |
| `x86_64-pc-windows-msvc` | Windows |

安装器：`shell` + `powershell`；`install-path = CARGO_HOME`；`install-updater = false`。

`dist plan` 为 CI 计算的构建矩阵：`macos-14`（aarch64-apple）、`macos-15-intel`（x86_64-apple）、`windows-2022`、
`ubuntu-22.04`（gnu）、`ubuntu-22.04`（musl）。以上为 2026-10-17 在本机用 cargo-dist 0.33.0 实测输出；musl 作业的
`packages_install` 仍会装 `musl-tools`，但实际编译器来自 `scripts/install-musl-toolchain.sh`。

**未纳入的目标（裁决，议题 [#106](https://github.com/Thatgfsj/webctl/issues/106)）**：`aarch64-unknown-linux-musl` 与 `aarch64-unknown-linux-gnu` **均未纳入 v0.1.x** —— 即 **ARM64 Linux 目前没有发布资产**。

- **为什么现在不做**：本仓 0 个 ARM64 需求证据；新增 1 个目标会把资产从 28 抬到 30+、多一个含 BoringSSL C++ 交叉编译的 CI 作业；`ubuntu-24.04-arm` runner 的可用性与配额尚未实测。
- **纳入条件（任一成立即开工）**：有真实 ARM64 使用诉求；或需要 Docker 多架构 / 边缘部署场景。
- **纳入时要做的**（避免将来重新论证）：① 扩展 `scripts/install-musl-toolchain.sh` 使其按三元组选择 Bootlin 工具链（同样 pin 版本 + SHA256）；② 把 `ci.yml` 的 `musl-static` 作业扩为矩阵；③ 在 `dist-workspace.toml` 加目标并使 `cargo dist generate --mode ci --check` = 0；④ tag 后回填体积基线；⑤ README 中英 / 本文件 / ACCEPTANCE 同步。

## 2. 安装（2026-10-26 实测，v0.2.0 真实资产）

| 资产 | 类型 | 内容 / 目标 |
|---|---|---|
| `webctl-x86_64-unknown-linux-musl.tar.xz` (+ `.sha256`) | 压缩包 | **v0.1.1 起产出**（33,864,888 字节（解压后二进制），`ldd` 报 `statically linked`）；`webctl-mcp` 同款 |
| `webctl-x86_64-unknown-linux-gnu.tar.xz` (+ `.sha256`) | 压缩包 | 同上（glibc） |
| `webctl-aarch64-apple-darwin.tar.xz` (+ `.sha256`) | 压缩包 | 同上 |
| `webctl-x86_64-apple-darwin.tar.xz` (+ `.sha256`) | 压缩包 | 同上 |
| `webctl-x86_64-pc-windows-msvc.zip` (+ `.sha256`) | 压缩包 | `webctl.exe` + `LICENSE` + `README.md` |
| `webctl-installer.sh` / `webctl-installer.ps1` | 安装器 | Linux/macOS shell · Windows PowerShell |
| `source.tar.gz` (+ `.sha256`)、`sha256.sum` | 源码 | 对应 tag 的源码与统一校验和 |
| `webctl-mcp-<target>.tar.xz` / `.zip` (+ `.sha256`) | 压缩包 | **第二个二进制**：`webctl-mcp`（独立 MCP server 入口；与 `webctl mcp` 子命令等价）。`dist plan` 对 workspace 内该包同样产出资产 |
| `webctl-mcp-installer.sh` / `.ps1` | 安装器 | 同上（对应 `webctl-mcp`） |

以下安装命令取自 **v0.2.0 的真实资产**（`dist plan` 的 `install_hint`；2026-10-26 实测可用）：

~~~bash
# Linux / macOS（v0.1.2）
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Thatgfsj/webctl/releases/download/v0.2.0/webctl-installer.sh | sh

# Windows PowerShell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/Thatgfsj/webctl/releases/download/v0.2.0/webctl-installer.ps1 | iex"
~~~

## 3. 发布步骤（维护者）

1. 定版本：编辑 `Cargo.toml` 的 `[workspace.package] version`（各 crate 用 `version.workspace = true`）。
2. 写 [CHANGELOG.md](../CHANGELOG.md)：把 `[Unreleased]` 改为 `[X.Y.Z] - YYYY-MM-DD`（见 §6）。
3. 本地门禁（见 §7）并确认真实输出；**再跑一次性能基线** `scripts/perf-baseline.sh --check`（L4 通道，抓量级级退化；有效门槛见 [TESTING §6.11](TESTING.md)）。**这一步必须在打 tag 之前完成**。
4. 提交、开 PR、由审阅者合并到 `main`。
5. 打 tag 并推送（tag 必须与 Cargo 版本一致）：

~~~bash
git switch main && git pull
git tag v0.1.1
git push origin v0.1.1
~~~

6. 观察 **Actions → Release** 流水线：`plan` → `build-local-artifacts` → `build-global-artifacts` → `host` → `announce`。
7. 按 §4 跑发布 E2E，并回填体积基线（§7）。
8. 失败时回滚后重来：

~~~bash
gh release delete v0.1.1 --yes       # 若 release 已建
git push origin :refs/tags/v0.1.1    # 删远端 tag
git tag -d v0.1.1                    # 删本地 tag
~~~

> **发布纪律**：打 tag 前，改动必须先经审阅者合并到 `main`。v0.1.0（2026-10-17）即按此流程发布（见 §4 与 ACCEPTANCE §7）。

## 4. 发布 E2E（PKG-04 / PKG-06 / PKG-07 / PKG-08）

**当前状态：已于 2026-10-17 执行**（tag `v0.1.1` 与 **28 个资产**已存在；musl 静态资产经 `sha256sum -c` + `ldd` + `release-e2e.sh --static` 验证通过，实测结果见 ACCEPTANCE §7）。收尾清单：

| 用例 | 命令 | 期望 |
|---|---|---|
| PKG-04 版本 | `./webctl --version` | 与 tag 一致（如 `webctl 0.1.0`） |
| PKG-06 干净环境 | 无 Rust、无浏览器的容器/新用户里跑 `./webctl caps --json` | `ok=true`、退出码 0 |
| PKG-08 无动态依赖 | `ldd ./webctl`（**musl 资产**） | **已达成（v0.1.1）**：`webctl-x86_64-unknown-linux-musl.tar.xz` 下载后 `ldd` 报 `statically linked`、`release-e2e.sh --static` PASS |
| PKG-05 体积 | `scripts/size-baseline.sh --binary ./webctl --target x86_64-unknown-linux-gnu --record` | 回填 glibc 基线 |
| PKG-07 文档一致 | 对照 README 安装段与本文件 §2 | README 不出现尚不存在的资产或命令 |

一键脚本（本地即可跑，自动覆盖 PKG-04/06/08）：

~~~bash
scripts/release-e2e.sh ./webctl 0.1.1          # glibc 资产
# musl 静态资产（v0.1.1 起已产出）：scripts/release-e2e.sh ./webctl 0.1.1 --static
~~~

干净环境示例（本机无 Docker 时可用新用户 / `env -i` 模拟；脚本本身不依赖 Rust）：

~~~bash
docker run --rm -v "$PWD:/w" -w /w debian:stable-slim ./webctl --version
#（musl 静态资产已随 v0.1.1 发布，并已在**真实 Alpine(musl) 用户态**验证：见 ACCEPTANCE §7 的 bwrap + minirootfs 方法）
~~~

## 5. Release note 模板

~~~markdown
## webctl vX.Y.Z

一句话摘要（这个版本解决了什么）。

### 安装
- Linux / macOS：见 docs/RELEASE.md §2 的 installer（或下载 webctl-<target>.tar.xz）
- Windows：下载 webctl-x86_64-pc-windows-msvc.zip

### 变更
#### Added
- …
#### Changed
- …
#### Fixed
- …
#### Security
- …

### 破坏性变更
- 无 / 说明迁移方式（参考 docs/MIGRATION.md）

### 已知限制与交接
- 明确列出未完成项与接手议题，不写"假装已支持"的能力。

### 校验和
- 附 sha256.sum 链接；逐资产 sha256 见各 .sha256 文件。
~~~

## 6. 变更日志约定（Keep a Changelog 风格）

- 仓库根 [CHANGELOG.md](../CHANGELOG.md)，遵循 [Keep a Changelog 1.1.0](https://keepachangelog.com/zh-CN/1.1.0/) 与 [SemVer](https://semver.org/lang/zh-CN/)。
- 分类固定为：`Added` / `Changed` / `Deprecated` / `Removed` / `Fixed` / `Security`。
- 未发布改动写入 `## [Unreleased]`；发版时改为 `## [X.Y.Z] - YYYY-MM-DD`，并新增对应的 compare 链接。
- 版本标题的数字必须与 tag 一致：cargo-dist 会把对应段落作为 GitHub Release 正文（`announcement_github_body`）。
- 不引入 changelog 生成工具，人工维护（议题 #23 明确"Keep a Changelog 风格即可，不必引工具"）。

## 7. 体积与冷启动基线（PKG-05、CLI-12）

- 脚本：`scripts/size-baseline.sh`；记录：`docs/size-baseline.json`。
- **v0.2.0 真实发布资产实测（2026-10-26）**：
  - glibc：**34,767,656 字节** / `--version` 冷启动 p50 **6 ms**（min 5 / max 7，10 次采样），`ldd` 动态；
  - musl：**34,122,368 字节** / 冷启动 p50 **5 ms**（min 4 / max 6），`ldd` 报 `statically linked`。
  - 均较 v0.1.2（34,496,768 / 33,864,888）增约 **0.8%**，远低于 10% 容差；基线已回填 `docs/size-baseline.json`（`source=release-asset`、`commit=b2185b3`）。
  - 历史：v0.1.2 的 glibc 34,496,768 字节（p50 6ms）、musl 33,864,888 字节（p50 4ms）仍保留在 git 历史与 CHANGELOG 中。
- 比较方法（体积超过基线 10% 或冷启动 p50 > 50 ms 时退出码 1）：

~~~bash
scripts/size-baseline.sh --binary ./webctl --target x86_64-unknown-linux-gnu --check
~~~

- 回填方法（重写 bytes / cold_start_ms_p50 / linkage / recorded_at / commit）：

~~~bash
scripts/size-baseline.sh --binary ./webctl --target x86_64-unknown-linux-gnu --record
~~~

## 8. 交接点与未做项（不假装完成）

| 项 | 状态 | 接手 |
|---|---|---|
| musl 静态构建 | **已达成（v0.1.1）**：发布资产经 `sha256sum -c` + `ldd`（statically linked）+ `release-e2e.sh --static` PASS；CI 的 `musl-static` 作业在 push main 时同样断言（PR 阶段不跑） | 完成 |
| 发布 E2E（PKG-04/06/08） | **已执行（v0.2.0，2026-10-26）**：4 个 Linux 压缩包 `sha256sum -c` 全过 + gnu/musl 资产 `release-e2e.sh` **PASS** + Alpine 用户态可跑，见 ACCEPTANCE §7 | 完成 |
| PKG-05 musl 体积数字 | **已回填（v0.2.0 真实资产）**：34,122,368 字节（解压后二进制） / 冷启动 p50 5ms；glibc 34,767,656 字节 / p50 6ms（`source=release-asset`、`commit=b2185b3`） | 完成 |
| PKG-07 文档-资产一致 | **已完成**：README / 本文件的安装命令与资产名已按 **v0.2.0** 真实资产逐项核对（28 个资产） | 完成 |
| `caps --json` 枚举工具 | 由 PR 11 / 议题 #68 落地（当时 15 个）；#114 起 16 个；0.2 M1 起 **17 个** | PR 11 / #114 |
| 代码签名 / macOS 公证 | 未配置（需 Apple Developer 凭据与 Windows 证书） | 后续议题 |
| `aarch64-unknown-linux-musl` | 未纳入 targets（交叉编译 BoringSSL 风险，先保守） | 后续议题 |
