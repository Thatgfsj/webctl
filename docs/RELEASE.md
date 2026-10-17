# webctl 发布流程（RELEASE）

| 项 | 值 |
|---|---|
| 适用 | v0.1.1（当前 release）及后续版本 |
| 流水线 | cargo-dist 0.33.0：`dist-workspace.toml` + `.github/workflows/release.yml` |
| 状态 | **v0.1.1 已发布**（[Release v0.1.1](https://github.com/Thatgfsj/webctl/releases/tag/v0.1.1)）：本文件是可执行的操作手册；发布 E2E 已于 2026-10-17 执行（见 §4 / ACCEPTANCE §7） |
| 相关 | [TESTING §2.16 PKG](TESTING.md) · [PLAN §5.2 PR 14](PLAN.md) · [体积基线](size-baseline.json) |

## 1. 流水线结构

- `dist-workspace.toml`：cargo-dist 配置，**唯一事实来源**。`release.yml` 由 `dist generate` 依据它生成；
  `dist generate --check` 可机械确认二者一致（本 PR 已通过，退出码 0）。
- `.github/workflows/release.yml`：cargo-dist 自动生成。触发条件为 push tag（形如 `**[0-9]+.[0-9]+.[0-9]+*`）或 pull request
  （PR 上只跑 `dist plan`，不发布、不建 release）。
- `.github/workflows/ci.yml`：每 PR 三平台门禁 + `musl-static` 作业（固定 Bootlin musl C++ 工具链，构建并断言静态）。

目标平台（`dist-workspace.toml` 的 `targets`）：

| target | 说明 |
|---|---|
| `x86_64-unknown-linux-musl` | Linux **静态**（v0.1.1 起随发布产出）：CI 每次 PR 用固定 Bootlin 工具链构建并断言静态 |
| `x86_64-unknown-linux-gnu` | Linux glibc 回退 |
| `aarch64-apple-darwin` / `x86_64-apple-darwin` | macOS（Apple Silicon / Intel） |
| `x86_64-pc-windows-msvc` | Windows |

安装器：`shell` + `powershell`；`install-path = CARGO_HOME`；`install-updater = false`。

`dist plan` 为 CI 计算的构建矩阵：`macos-14`（aarch64-apple）、`macos-15-intel`（x86_64-apple）、`windows-2022`、
`ubuntu-22.04`（gnu）、`ubuntu-22.04`（musl）。以上为 2026-10-17 在本机用 cargo-dist 0.33.0 实测输出；musl 作业的
`packages_install` 仍会装 `musl-tools`，但实际编译器来自 `scripts/install-musl-toolchain.sh`。

## 2. 发布物（`dist plan` 实测，2026-10-17，`webctl 0.1.0`）

| 资产 | 类型 | 内容 / 目标 |
|---|---|---|
| `webctl-x86_64-unknown-linux-musl.tar.xz` (+ `.sha256`) | 压缩包 | **v0.1.1 起产出**（22,612,520 字节，`ldd` 报 `statically linked`）；`webctl-mcp` 同款 |
| `webctl-x86_64-unknown-linux-gnu.tar.xz` (+ `.sha256`) | 压缩包 | 同上（glibc） |
| `webctl-aarch64-apple-darwin.tar.xz` (+ `.sha256`) | 压缩包 | 同上 |
| `webctl-x86_64-apple-darwin.tar.xz` (+ `.sha256`) | 压缩包 | 同上 |
| `webctl-x86_64-pc-windows-msvc.zip` (+ `.sha256`) | 压缩包 | `webctl.exe` + `LICENSE` + `README.md` |
| `webctl-installer.sh` / `webctl-installer.ps1` | 安装器 | Linux/macOS shell · Windows PowerShell |
| `source.tar.gz` (+ `.sha256`)、`sha256.sum` | 源码 | 对应 tag 的源码与统一校验和 |
| `webctl-mcp-<target>.tar.xz` / `.zip` (+ `.sha256`) | 压缩包 | **第二个二进制**：`webctl-mcp`（独立 MCP server 入口；与 `webctl mcp` 子命令等价）。`dist plan` 对 workspace 内该包同样产出资产 |
| `webctl-mcp-installer.sh` / `.ps1` | 安装器 | 同上（对应 `webctl-mcp`） |

以下安装命令取自 **v0.1.1 的真实资产**（`dist plan` 的 `install_hint`；已于 2026-10-17 实测可用）：

~~~bash
# Linux / macOS（tag 为 v0.1.1 时）
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Thatgfsj/webctl/releases/download/v0.1.1/webctl-installer.sh | sh

# Windows PowerShell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/Thatgfsj/webctl/releases/download/v0.1.1/webctl-installer.ps1 | iex"
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

**当前状态：已于 2026-10-17 执行**（tag `v0.1.1` 与 26 个资产已存在；musl 静态资产经 `sha256sum -c` + `ldd` + `release-e2e.sh --static` 验证通过，实测结果见 ACCEPTANCE §7）。收尾清单：

| 用例 | 命令 | 期望 |
|---|---|---|
| PKG-04 版本 | `./webctl --version` | 与 tag 一致（如 `webctl 0.1.0`） |
| PKG-06 干净环境 | 无 Rust、无浏览器的容器/新用户里跑 `./webctl caps --json` | `ok=true`、退出码 0 |
| PKG-08 无动态依赖 | `ldd ./webctl` | **构建侧已达成（CI）**：`musl-static` 作业断言 `file`/`ldd`/`readelf` 无动态依赖；release 资产待下一次 tag（[#90](https://github.com/Thatgfsj/webctl/issues/90)） |
| PKG-05 体积 | `scripts/size-baseline.sh --binary ./webctl --target x86_64-unknown-linux-gnu --record` | 回填 glibc 基线 |
| PKG-07 文档一致 | 对照 README 安装段与本文件 §2 | README 不出现尚不存在的资产或命令 |

一键脚本（本地即可跑，自动覆盖 PKG-04/06/08）：

~~~bash
scripts/release-e2e.sh ./webctl 0.1.0          # glibc 资产
# musl 静态资产（下一次 tag 起）：scripts/release-e2e.sh ./webctl 0.1.0 --static
~~~

干净环境示例（本机无 Docker 时可用新用户 / `env -i` 模拟；脚本本身不依赖 Rust）：

~~~bash
docker run --rm -v "$PWD:/w" -w /w debian:stable-slim ./webctl --version
#（musl 静态资产发布后（下一次 tag）可用 alpine 验证干净环境，见 [#90](https://github.com/Thatgfsj/webctl/issues/90)）
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
- **本机实测（glibc release，2026-10-17，commit 5f10aa6）**：**7,978,256 字节（约 7.61 MiB）**；
  `--version` 冷启动 p50 实测在 **10–24 ms** 之间波动（每轮 10 次采样；min 9 / max 179 为首次冷缓存），全部 **< 50 ms，CLI-12 本地通过**（基线文件记录 p50 = 24 ms）。
- **musl 数字：22,612,520 字节 / 冷启动 p50 4ms** —— 取自 v0.1.1 真实发布资产（`ldd` 报 `statically linked`）。
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
| musl 静态构建 | **CI 已达成**：`musl-static` 作业每次 PR 构建 `webctl`/`webctl-mcp` 并断言静态；发布矩阵已恢复。剩余：待下一次 tag 产出 release 资产 | [#90](https://github.com/Thatgfsj/webctl/issues/90) |
| 发布 E2E（PKG-04/06/08） | **已执行（2026-10-17）**：下载 + `sha256sum -c` 校验 + `release-e2e.sh` **PASS**，见 ACCEPTANCE §7 | 完成 |
| PKG-05 musl 体积数字 | 待真实 release 资产回填（CI 构建值不写入基线） | [#90](https://github.com/Thatgfsj/webctl/issues/90) |
| PKG-07 文档-资产一致 | **已完成**：README 安装命令与资产名已按 **v0.1.1** 真实资产逐项核对 | 完成 |
| `caps --json` 枚举 15 工具 | 由 PR 11 / 议题 #68 落地；README 已留回填标记 | PR 11 |
| 代码签名 / macOS 公证 | 未配置（需 Apple Developer 凭据与 Windows 证书） | 后续议题 |
| `aarch64-unknown-linux-musl` | 未纳入 targets（交叉编译 BoringSSL 风险，先保守） | 后续议题 |
