# v0.1 验收清单（ACCEPTANCE）

| 项 | 值 |
|---|---|
| 版本 | v0.1（对应 [PLAN v0.4](PLAN.md)） |
| 用途 | 收尾时**逐条走**的账：谁跑什么、期望什么、证据放哪 |
| 状态 | 维护中：PR 03–14 全部合并后逐条勾选 |
| 相关 | [PLAN §5.2.1 交付记录](PLAN.md)、[TESTING 用例清单](TESTING.md)、[CONTRIBUTING 门禁](CONTRIBUTING.md) |

## 0. 怎么用

1. 每条都有**可机械执行**的命令与**可核对**的期望；不接受「看起来没问题」。
2. 证据统一贴在对应 PR 评论或本节下方的「验收记录」里。
3. 未达成的项**不许勾选**：要么修，要么进 §6「已知延期登记」并写明原因与接手议题。

## 1. 每个 PR 的交付门禁（§5.2 十四个 PR）

每个 PR 合并前必须同时满足：

- [ ] `cargo fmt --check` → exit 0
- [ ] `cargo clippy --all-targets -- -D warnings` → 0 warning
- [ ] `cargo test` → 全绿
- [ ] `cargo test --features offline` → 全绿（L0 离线 lane）
- [ ] **三平台 CI 全 success**（ubuntu / macos / windows），且**确认 CI 真的触发了**（冲突会导致 merge ref 缺失、CI 静默不跑——PR 03 踩过）
- [ ] 独立审阅者复核通过并**由审阅者**合并（实现者不自行合并）
- [ ] 行数如实申报（§5.1；启动期不作门禁，见议题 #41）

各 PR 的状态与 merge sha 见 [PLAN §5.2.1](PLAN.md)。

## 2. 四条通道（TESTING §1）

| 通道 | 命令 | 期望 |
|---|---|---|
| **L0 离线** | `cargo test --features offline` | 取回/解析/会话/文档/抽取用例全绿；**不访问外网**（fixture 只绑 127.0.0.1） |
| **L1 真站 smoke** | `./tests/e2e/run-real.sh` | 可跳过；静态锚点取到 10 条；JS 页 `DEGRADED`。**不作 CI 门禁** |
| **L2 体积/静态** | `cargo build --release` + `scripts/size-baseline.sh --check`；musl 由 CI `musl-static` 作业与**发布资产**双重验证 | 体积基线记录在案（PKG-05，glibc + musl 两条）；musl 静态**已达成**（v0.1.1 资产 + `release-e2e.sh --static` PASS） |
| **L3 安全负面** | 默认配置下跑安全用例 | 越权/越界/协议滥用全部被拒（SEC-01…15） |

## 3. 三条验收锚点（PLAN §5.3）

- [ ] **诚实降级锚点**：JS 页（`quotes.toscrape.com/js/`）返回 `DEGRADED` 或 `UNSUPPORTED`，**不得**「成功但 0 条」
- [ ] **静态等价锚点**：静态版（`quotes.toscrape.com/`）稳定取到 **10 条**
- [ ] **会话锚点**：本地表单站「登录 → 翻页 → 提交」端到端通过

## 4. 工具面与打包

- [x] `caps --json` **如实反映**已实现能力，并**枚举 PLAN §3.7 的 15 个工具**（议题 #68 已关闭）—— 实测 5 个域 available + 枚举 15 个工具
- [x] MCP `tools/list` 恰好 15 个且与 §3.7 逐字同序（MCP-02）；`check-skill-tools.sh` 校验 SKILL.md 一致（MCP-12）
- [x] 服务器指令 2222 字节 ≤ 32768（MCP-05）；配置不写 `toolOrder`（MCP-08）
- [x] Release 资产在**干净环境**可下载即用（PKG-06）：**v0.1.1** 资产（glibc 与 **musl 静态**）下载 + `sha256sum -c` 校验通过 + `scripts/release-e2e.sh`（musl 加 `--static`）PASS；musl 资产另在**真实 Alpine(musl) 用户态**实测（§7）
- [x] README 双语；安装命令已改为 **v0.1.1 真实资产**的 URL（不再声明「尚无发布版」）；无虚构徽章（PKG-07）
- [x] 体积基线已从**v0.1.1 真实发布资产**回填（PKG-05）：glibc **22,867,752** 字节 / p50 6ms；musl 静态 **22,612,520** 字节 / p50 4ms（见 `docs/size-baseline.json`）

## 5. 硬约束（全仓审计，收尾时各跑一次）

- [ ] **零外部进程**：`crates/**/src/**` 无 `std::process`/`Command`（仅 `ExitCode`）；offline lane 的静态扫描 0 命中
- [ ] **无 unsafe**：workspace `unsafe_code = "forbid"` 生效，`git grep -n "unsafe " -- crates/` 无实质命中
- [ ] **零浏览器**：无 playwright/CDP/Edge/Chrome 依赖与调用（ADR-0001）
- [ ] **禁止静默降级**：任何 `degraded_from` 非空的信封，其 `code != OK`（`Envelope::validate`）
- [ ] **信封封闭集**：`code` 只出现在 11 个错误码内
- [ ] **信任标记**：页面内容一律 `meta.trust == untrusted_page_content`

## 6. 已知延期登记（必须显式，不许悄悄消失）

| 项 | 现状 | 接手 |
|---|---|---|
| 代理 DX：无 `NO_PROXY` 支持、无 `--no-proxy` | PR 10 审阅结论里记录为非阻塞 | 待开议题 |
| POLITE-06 缺仓库内 e2e 用例 | PR 08 复审建议 | 待开议题 |
| 扫描器残余绕过（`Command :: new(`、`C::new(` 别名/分组导入） | README 已如实记录 | 低优先 |
| flock 覆盖边界：持锁进程被 kill 后的内核释放、NFS 语义 | PR 05 模块文档已如实标注 | 不承诺支持 NFS |
| `sessions_dir()` 仍是未校验的 public path getter | PR 05 复审：当前生产代码不可达（落盘/读盘均走 checked 入口），但属 API 隐患 | 建议改为返回 `Result` 或 `pub(crate)` |
| `PathError::Io` 被映射成 `UnsafePath(Blocked)` | PR 05 复审：`sessions` 为普通文件时 `load`=Blocked 而 `list`=Internal，分类不一致（均 fail-closed） | 待开议题，低优先 |
| 路径校验与 IO 之间的 TOCTOU 窗口（无 `openat`/`O_NOFOLLOW`） | PR 05 复审：需同 UID 且在 0700 目录下并发篡改才可利用，非实际边界破坏 | 记录在案，不承诺修复 |
| extract 的 `links`/`tables`/`forms`/`form_by_index` 未纳入 `max_elapsed_ms` 兜底 | PR #71 复审登记：`links` 只在过滤循环查预算（解析耗时不计入、零链接时循环体不执行）；`tables`/`forms`/`form_by_index` 完全不读取预算 | 建议随 #73（PR 09 会重写 `links`/`structured`）或后续议题一并收口 |
| `download` / `doc_text` 的 URL 路径仅支持文本类响应 | PR 02 的 `Fetched.content` 不暴露二进制原始字节；二进制返回 `UNSUPPORTED` 降级信封 | 需在 webctl-web 加原始字节接口（PR 11 正文已登记） |
| 解析/抽取性能 `[~5s, 30s)` 档 | **已由 L4 通道接手**：`scripts/perf-baseline.sh --check`（相对基线 + 5× tolerance，抓**量级级退化**，TESTING §6.11）。诚实边界：**不抓 20% 级抖动**，也不替代单测挂死守卫 | 议题 [#83](https://github.com/Thatgfsj/webctl/issues/83)（本轮落地） |
| musl 静态目标（PKG-02 / PKG-08） | **已达成（v0.1.1）**：发布矩阵产出 `webctl-x86_64-unknown-linux-musl.tar.xz`（22,612,520 字节）与 `webctl-mcp` 同款；`sha256sum -c` 通过、`ldd` = `statically linked`、`release-e2e.sh --static` PASS；并已在**真实 Alpine(musl) 用户态**实测（user namespace + bwrap + minirootfs，见 §7） | 议题 [#90](https://github.com/Thatgfsj/webctl/issues/90)（已收口） |

## 7. 验收记录

> 收尾时在下面追加：日期、执行者、命令、结果摘要、CI run 链接。

- 2026-10-16（Lead）main `1f45dcd` 本地复核：`fmt` OK、`clippy` 0、**241 用例全绿**、`--features offline` 全绿。

- 2026-10-16（Lead）main `820df36` **硬约束全仓审计**（一手证据）：
  - **零外部进程**：`crates/*/src/` 内无 `Command::new`/`.spawn(`；仅 `std::process::id()`（`config.rs`，用于临时路径唯一性）与 `use std::process::ExitCode`（bin ×2），均不创建进程。
  - **无 unsafe**：`crates/*/src/` 命中 3 处，全在注释/错误文案里；workspace `unsafe_code = "forbid"` 生效。
  - **零浏览器**：`Cargo.toml`/`crates/*/Cargo.toml` 无 playwright/chromium/cdp/selenium；`Cargo.lock` 无 v8/quickjs/boa_engine。
  - **错误码封闭集**：`ErrorCode` 共 **11** 个变体。
  - **静态扫描**：`cargo test --features offline --test offline_lane` → 4 passed / 2 ignored（两个扫描器 0 命中）。
  - 备注：手写 `grep 'http://\|https://'` 会报 30 处"疑似外站"，而正规扫描器 0 命中——差异来自保留域/模板主机/回环规则，说明扫描器的白名单规则是必要的。
- 2026-10-17（Lead）**DOC-09 收口**（议题 #59，PR #84）：
  - `doc_text` 的 HTML 分支改为调用 `parse::extract_article`（正文纯文本）；XML 分支调用 `parse::parse_xml` 后**只收文本节点**（跳过 `tag`/`attrs`，不把标记结构混进正文）；
  - 交接期的 `DOC09_DEFERRED_DEGRADED_FROM`（`parse-layer-pending-pr03`）与过渡期边界用例**已从代码与测试中彻底删除**；
  - `doc_09_acceptance_after_pr03` 去掉 `#[ignore]` 后**通过**（原过渡用例里的「纯文本直通」断言移入其中，覆盖不减）。

  ⇒ §6 中该条延期登记**已注销**。

- 2026-10-17（Lead）**收尾验收走查（main 上 PR 03–14 全合并后）**：
  - **L0 离线通道**：`cargo test --features offline` → **347 passed / 0 failed / 3 ignored**；
  - **L1 真站锚点**（`WEBCTL_REAL=1 crates/webctl-web/tests/e2e/run-real.sh`，EXIT=0）：静态版取到 **10 条**；JS 版返回**非 OK 的降级码且 `degraded_from` 非空**；
  - **工具面**：`webctl mcp --list-tools` → **恰好 15 个**且与 §3.7 逐字同序；`webctl caps --json` → 5 个域 available 且**枚举 15 个工具**；`scripts/check-skill-tools.sh` → 「SKILL.md 与 tools/list 一致（15 个工具）」；
  - **硬约束全仓审计**：`crates/*/src/` 内 `Command::new`/`.spawn(` **0**、unsafe 实质命中 **0**、浏览器/JS 引擎依赖 **0**、错误码 **11** 个；
  - **收口**：DOC-09（#59）与 caps 诚实性（#68）已关闭；state_02/state_03 计时 flake 修复（#85 → #86）。
  - 尚待：**release E2E**（打 tag → 下载资产 → 干净环境运行），见 §4。

- 2026-10-17（Lead）**release E2E 实测（v0.1.0，首个真实发布）**：
  - 打 tag `v0.1.0`（annotated，含发布说明）→ Release 流水线 run `39012230617` **全绿**：plan / build-local-artifacts ×4（gnu、aarch64-apple、x86_64-apple、windows-msvc）/ build-global-artifacts / host / **announce** 均 success；
  - **4 个目标全部构建成功**（musl 已于本日摘除，见议题 [#90](https://github.com/Thatgfsj/webctl/issues/90)）；此为 v0.1.0 历史记录，musl 构建侧已由 PR #95 恢复 CI 验证。
  - Release 已创建：[https://github.com/Thatgfsj/webctl/releases/tag/v0.1.0](https://github.com/Thatgfsj/webctl/releases/tag/v0.1.0)，资产含 `webctl`/`webctl-mcp` 各 4 平台压缩包 + 各自 `.sha256`、shell/PowerShell 安装器、源码包与 `sha256.sum`；
  - **下载验证**：`gh release download` → `sha256sum -c` **成功** → 解压 → `scripts/release-e2e.sh ./webctl 0.1.0` **PASS**（PKG-04 版本 == tag；PKG-06 干净环境 `caps --json` 可跑、5 个域；PKG-08 `ldd` 显示 glibc 动态依赖，与文档一致）；
  - **体积/冷启动回填**：CI 产物 22,859,144 字节、冷启动 p50 5ms（本机同提交 release profile 21,067,504、dist profile 22,891,304，故 CI 值合理）。与旧基线 7,978,256（commit `5f10aa6`，PR 09/11/12 合并**前**）相比增长约 2.9×，来自 MCP（rmcp+schemars）、降级语义与安全层，**非打包异常**。

- 2026-10-17（PR #95，议题 #90）**musl 静态目标 CI 验证（构建侧）**：
  - 新增 `musl-static` 作业（ubuntu-latest），使用固定 Bootlin 2026.08-1 musl C++ 工具链（GCC 15.3.0 / musl 1.2.6，SHA256 校验），构建 `webctl`/`webctl-mcp`；
  - CI run [39019246132](https://github.com/Thatgfsj/webctl/actions/runs/39019246132)（commit `79545b3`）**musl-static success**：构建 3m53s；两个二进制均 `file=static-pie linked`、`ldd=statically linked`、`readelf -d` 无 NEEDED；`webctl --version` = 0.1.0、`caps --json` 可运行；
  - 本机同工具链实测（Debian 13）：`webctl` 20,492,720 B、`webctl-mcp` 19,395,664 B（未 strip；**不写入体积基线**）；
  - **仍未达成**：v0.1.0 没有 musl release 资产，PKG-05 musl 数字仍为 null；下一次 tag 由维护者执行后回填。

- 2026-10-17（Lead）**v0.1.1 发布 E2E（musl 静态资产首次产出）**：
  - 打 tag `v0.1.1`（annotated，含发布说明）→ Release 流水线 run `39025412524` **全绿**：plan / build-local-artifacts ×**5**（gnu、**musl**、aarch64-apple、x86_64-apple、windows-msvc）/ build-global-artifacts / host / **announce** 均 success；
  - Release 已创建：[https://github.com/Thatgfsj/webctl/releases/tag/v0.1.1](https://github.com/Thatgfsj/webctl/releases/tag/v0.1.1)，**28 个资产**（5 个目标 ×（`webctl` 与 `webctl-mcp` 的压缩包 + 各自 `.sha256`）= 20；安装器 4：`webctl-installer.{sh,ps1}` 与 `webctl-mcp-installer.{sh,ps1}` = 24；`source.tar.gz` + `source.tar.gz.sha256` = 26；`sha256.sum` = 27；`dist-manifest.json` = 28）；
  - **musl 静态验证（#90 的验收核心）**：下载 `webctl-x86_64-unknown-linux-musl.tar.xz` → `sha256sum -c` **成功** → `file`/`ldd` 报 **statically linked** → `scripts/release-e2e.sh --static` **PASS**（PKG-04 版本 == tag；PKG-06 干净环境 `caps --json`；**PKG-08 静态链接**）；
  - **体积/冷启动基线回填（真实资产）**：musl **22,612,520** 字节 / p50 **4ms**（static）；glibc **22,867,752** 字节 / p50 6ms（dynamic）——两条 `source` 均为 `release-asset`；
  - **Alpine / 容器干净环境 E2E**：v0.1.1 发布当日**已闭环**——本机虽无 Docker/podman，但用 user namespace + `bwrap` + Alpine minirootfs 构造真实 musl 用户态并实测通过（方法见本节下一条；议题 [#105](https://github.com/Thatgfsj/webctl/issues/105) 已关闭）。

- 2026-10-17（Lead）**Alpine(musl) 干净环境 E2E（#105，方法：user namespace + bwrap）**：
  - 本机无 Docker/podman，改用 `unshare` 的用户命名空间 + `bwrap` 构造真实 Alpine rootfs（`alpine-minirootfs-3.20.3-x86_64`，`/lib/libc.musl-x86_64.so.1` 存在 ⇒ 真实 musl 用户态），并把 v0.1.1 的 musl 资产 bind 进去；
  - `webctl --version` → `webctl 0.1.1`；`caps --json` → `ok=true`、`engine=builtin`；
  - `webctl fetch https://quotes.toscrape.com/` → **HTTP/2 200**（TLS 在 musl 用户态可用）；
  - MCP over stdio（`webctl mcp`）→ `serverInfo={webctl, 0.1.1}`、**15 个工具**；**L1 锚点 1**：`extract` 静态页 → `code=OK`、**matched 10 element(s)**；**L1 锚点 2**：`extract` JS 页 → `code=DEGRADED`（诚实降级，不返回"成功但 0 条"）；
  - 失败路径也记一笔：不绑定 `/etc/resolv.conf` 时 Alpine rootfs 无解析配置 ⇒ `fetch` 返回 `NETWORK`；这是**环境**缺文件，不是 webctl 缺陷。
