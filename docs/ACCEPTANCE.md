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
| DOC-09：`doc_text` 的 HTML→文本切到解析层 | 保持显式 `UNSUPPORTED(parse-layer-pending-pr03)` + `#[ignore]` 验收用例；PR 03 已合并，交接条件已满足 | 议题 [#59](https://github.com/Thatgfsj/webctl/issues/59) |
| `caps` 诚实性（不谎报可用 + 枚举 15 工具） | 当前硬编码 available | 议题 [#68](https://github.com/Thatgfsj/webctl/issues/68)，归 PR 11，PR 14 核验 |
| 代理 DX：无 `NO_PROXY` 支持、无 `--no-proxy` | PR 10 审阅结论里记录为非阻塞 | 待开议题 |
| POLITE-06 缺仓库内 e2e 用例 | PR 08 复审建议 | 待开议题 |
| 扫描器残余绕过（`Command :: new(`、`C::new(` 别名/分组导入） | README 已如实记录 | 低优先 |
| flock 覆盖边界：持锁进程被 kill 后的内核释放、NFS 语义 | PR 05 模块文档已如实标注 | 不承诺支持 NFS |
| `sessions_dir()` 仍是未校验的 public path getter | PR 05 复审：当前生产代码不可达（落盘/读盘均走 checked 入口），但属 API 隐患 | 建议改为返回 `Result` 或 `pub(crate)` |
| `PathError::Io` 被映射成 `UnsafePath(Blocked)` | PR 05 复审：`sessions` 为普通文件时 `load`=Blocked 而 `list`=Internal，分类不一致（均 fail-closed） | 待开议题，低优先 |
| 路径校验与 IO 之间的 TOCTOU 窗口（无 `openat`/`O_NOFOLLOW`） | PR 05 复审：需同 UID 且在 0700 目录下并发篡改才可利用，非实际边界破坏 | 记录在案，不承诺修复 |
| extract 的 `links`/`tables`/`forms`/`form_by_index` 未纳入 `max_elapsed_ms` 兜底 | PR #71 复审登记：`links` 只在过滤循环查预算（解析耗时不计入、零链接时循环体不执行）；`tables`/`forms`/`form_by_index` 完全不读取预算 | 建议随 #73（PR 09 会重写 `links`/`structured`）或后续议题一并收口 |
| `parse_14_budget_on_100kb` 负载敏感（高负载下偶发超时） | PR 09 实现者报告；单跑/复跑均绿 | 按 #74/#75 的做法改为**不依赖绝对墙钟**的断言（或明确其 lane） |

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
