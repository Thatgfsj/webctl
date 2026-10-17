# 变更日志

本文件遵循 [Keep a Changelog 1.1.0](https://keepachangelog.com/zh-CN/1.1.0/) 与
[语义化版本](https://semver.org/lang/zh-CN/)。格式约定、发版步骤与 release note 模板见
[docs/RELEASE.md](docs/RELEASE.md)。

## [Unreleased]

### Added

- 打包与发布流水线：cargo-dist 配置（`dist-workspace.toml`）与自动生成的
  `.github/workflows/release.yml`；三平台资产（Linux / macOS / Windows）+ musl 静态优先（#23）。
- 体积与冷启动基线工具 `scripts/size-baseline.sh` 及基线记录 `docs/size-baseline.json`（PKG-05）。
- 发布资产 E2E 冒烟脚本 `scripts/release-e2e.sh`（PKG-04 / PKG-06 / PKG-08）。
- 双语 README（中文为主 + 英文摘要）与 [docs/RELEASE.md](docs/RELEASE.md) 发布手册（PKG-07）。

### 已知限制

- **musl 静态目标暂缓**：BoringSSL（C++）需要面向 musl 的 libstdc++，而 Debian/Ubuntu 的 `musl-dev` 只提供 C 侧；
  三次真实 release 均失败（缺交叉 C++ 编译器 → 链接用宿主 cc → `fatal error: memory: No such file`）。
  因此 Linux 发布物为 **glibc 动态链接**；静态目标见议题 [#90](https://github.com/Thatgfsj/webctl/issues/90)。
> 首个版本 v0.1.0 **尚未发布**：本节内容将在打 tag 时迁入 `## [0.1.0] - YYYY-MM-DD`。
