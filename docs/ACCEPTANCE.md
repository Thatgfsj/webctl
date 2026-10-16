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
| **L2 体积/静态** | `cargo build --release --target x86_64-unknown-linux-musl` | musl 静态产物可运行；体积基线记录在案（PKG-05） |
| **L3 安全负面** | 默认配置下跑安全用例 | 越权/越界/协议滥用全部被拒（SEC-01…15） |

## 3. 三条验收锚点（PLAN §5.3）

- [ ] **诚实降级锚点**：JS 页（`quotes.toscrape.com/js/`）返回 `DEGRADED` 或 `UNSUPPORTED`，**不得**「成功但 0 条」
- [ ] **静态等价锚点**：静态版（`quotes.toscrape.com/`）稳定取到 **10 条**
- [ ] **会话锚点**：本地表单站「登录 → 翻页 → 提交」端到端通过

## 4. 工具面与打包

- [ ] `caps --json` **如实反映**已实现能力，并**枚举 PLAN §3.7 的 15 个工具**（议题 #68）
- [ ] MCP `tools/list` 恰好 15 个工具且与 §3.7 名单一致（MCP-02），SKILL.md 与之一致（MCP-12）
- [ ] 服务器指令 ≤ 32768 字节（MCP-05）；配置不写 `toolOrder`（MCP-08）
- [ ] Release 资产在**干净环境**可下载即用：`--version` 与 `caps --json` 可跑（PKG-06）
- [ ] README 双语、无虚构徽章/命令；安装命令与实际发布物一致（PKG-07）
- [ ] 体积基线写入防回归（PKG-05）

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

| MCP 路径的 JS 降级传播未接线 | PR 11 留了显式交接点（常量 `DEGRADE_HANDOFF_PENDING_PR09` + `#[ignore]` 用例，主体是 panic 哨兵）；PR 09 已合并，条件已满足 | 议题 [#82](https://github.com/Thatgfsj/webctl/issues/82) |
| MCP 侧 SEC-09 的 `redact_fetched` 接入未做 | PR 12 在 `crates/webctl-mcp/src/lib.rs` 留 TODO；PR 11 自身只对敏感头/cookie 值脱敏 | 建议随 #82 一并补，并加 MCP 侧断言 |
| `download` / `doc_text` 的 URL 路径仅支持文本类响应 | PR 02 的 `Fetched.content` 不暴露二进制原始字节；二进制返回 `UNSUPPORTED` 降级信封 | 需在 webctl-web 加原始字节接口（PR 11 正文已登记） |
| 解析/抽取存在性能盲区 `[~5s, 30s)` | #80 把绝对墙钟改成 30s 挂死守卫后该区间无自动门禁（TESTING §1 的 L2 是发布物体积，不含解析性能） | 议题 [#83](https://github.com/Thatgfsj/webctl/issues/83)：建解析/抽取基准 |

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
  - 尚待：**release E2E**（打 tag → 下载资产 → 干净环境运行）与 musl 体积回填，见 §4。
