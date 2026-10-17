# 变更日志

本文件遵循 [Keep a Changelog 1.1.0](https://keepachangelog.com/zh-CN/1.1.0/) 与
[语义化版本](https://semver.org/lang/zh-CN/)。格式约定、发版步骤与 release note 模板见
[docs/RELEASE.md](docs/RELEASE.md)。

## [Unreleased]

### Added

- CI 增加 `musl-static` 作业：每次 PR 用固定版本 Bootlin 2026.08-1 musl C++ 工具链
  （GCC 15.3.0 / musl 1.2.6，SHA256 校验）构建 `webctl`/`webctl-mcp`，并用
  `file`/`ldd`/`readelf` 断言静态（[#90](https://github.com/Thatgfsj/webctl/issues/90)）。
- `x86_64-unknown-linux-musl` 恢复到 cargo-dist 发布矩阵；**下一次 tag** 才会产出
  musl 资产，v0.1.0 已有资产不变。

### Changed

- README（中英）、RELEASE、ACCEPTANCE、TESTING 与体积基线同步为「CI 已达成构建/静态断言，
  release 资产待下一次 tag；体积待真实资产回填」，不把 CI 构建值写成发布基线。

## [0.1.0] - 2026-10-17

### Added

- 打包与发布流水线：cargo-dist 配置（`dist-workspace.toml`）与自动生成的
  `.github/workflows/release.yml`；三平台资产（Linux / macOS / Windows）；**musl 静态目标暂缓**（见下方「已知限制」与 [#90](https://github.com/Thatgfsj/webctl/issues/90)）。
- 体积与冷启动基线工具 `scripts/size-baseline.sh` 及基线记录 `docs/size-baseline.json`（PKG-05）。
- 发布资产 E2E 冒烟脚本 `scripts/release-e2e.sh`（PKG-04 / PKG-06 / PKG-08）。
- 双语 README（中文为主 + 英文摘要）与 [docs/RELEASE.md](docs/RELEASE.md) 发布手册（PKG-07）。

### 已知限制

- **musl 静态目标暂缓**：BoringSSL（C++）需要面向 musl 的 libstdc++，而 Debian/Ubuntu 的 `musl-dev` 只提供 C 侧；
  三次真实 release 均失败（缺交叉 C++ 编译器 → 链接用宿主 cc → `fatal error: memory: No such file`）。
  因此 Linux 发布物为 **glibc 动态链接**；静态目标见议题 [#90](https://github.com/Thatgfsj/webctl/issues/90)。
> v0.1.0 已于 2026-10-17 发布：[https://github.com/Thatgfsj/webctl/releases/tag/v0.1.0](https://github.com/Thatgfsj/webctl/releases/tag/v0.1.0)。三平台资产（`webctl` 与 `webctl-mcp`）+ shell/PowerShell 安装器；Linux 为 glibc 动态链接。
