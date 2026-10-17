# DSH MCP 集成合规核对

本文件逐条核对 [SURVEY §1.4 / §8.6](SURVEY.md) 记录的 **DSH 侧硬约束**与 webctl MCP server 的实现，
每条给出「代码位置 → 验证方式 → 结论」。**实测用 v0.1.0 的真实发布资产**做探针（不是"大概没问题"）。

| 项 | 值 |
|---|---|
| 被核对对象 | `webctl mcp`（stdio JSON-RPC；等价二进制 `webctl-mcp`） |
| 探针二进制 | v0.1.0 真实 release 资产 `webctl-x86_64-unknown-linux-gnu`（`sha256sum -c` 通过） |
| 核对日期 | 2026-10-17 |

## 1. 约束核对表

| # | DSH 约束 | 代码位置 | 验证方式 | 结论 |
|---|---|---|---|---|
| 1 | `toolCallTimeoutMs` 默认 **60,000ms** | 默认请求预算 `timeout_ms = 30_000`（[`crates/webctl-core/src/config.rs`](../crates/webctl-core/src/config.rs)）；`paginate` 分片 | 默认预算 30s < 60s；长任务用 `paginate` 分片而非单次长跑 | **通过**（默认在预算内；分片策略见 §3） |
| 2 | `maxInstructionBytes` 默认 **32,768** | [`crates/webctl-mcp/src/instructions.rs`](../crates/webctl-mcp/src/instructions.rs)：`MAX_INSTRUCTIONS_BYTES = 32 * 1024` | 实测 `initialize` 响应 `instructions` = **2222 字节**（**v0.1.0 资产**，见 §2.1；文本改动后数字随之变化，余量仍 >14×） | **通过** |
| 3 | stdio 协商：**先起探测进程，再起服务进程** | 无全局锁/单实例限制；状态只在 state dir（cookie 会话） | 实测"探测→服务"与"两个并发服务"四种启动全部正常应答（见 §2.2） | **通过** |
| 4 | 连接失败：harness 仍启动但**工具不出现** | — | 排查法见 §4 | **已记录**（属 DSH 侧行为） |
| 5 | 工具名 `mcp__<serverName>__<rawName>` | [`crates/webctl-mcp/src/tools.rs`](../crates/webctl-mcp/src/tools.rs)：`TOOL_NAMES` 15 个**裸名** | `webctl mcp --list-tools` 输出 15 个裸名（`fetch` / `extract` / …） | **通过**（前缀由 DSH 施加，webctl 不自行加前缀） |
| 6 | 会话归属：单 provider，绑定确切 Agent+Session，跨 Session 拒绝 | [`crates/webctl-mcp/src/ownership.rs`](../crates/webctl-mcp/src/ownership.rs)：`SessionBinding` + `OwnershipError`；`resolve()` 在绑定不符时返回「归属拒绝」 | `ownership.rs` 内单元测试覆盖"首次绑定 / 同 Agent 通过 / 换 Agent 拒绝"（文件内含 `assert!` 断言与 `归属拒绝` 文案核对） | **通过** |
| 7 | `toolOrder`：**不要**把 webctl 工具名显式列进去 | [`instructions.rs`](../crates/webctl-mcp/src/instructions.rs)：`dsh_config_snippet()` 生成的配置**不含** `toolOrder`，也不枚举工具名 | `crates/webctl-mcp/tests/mcp.rs` 断言 `!instructions.contains("toolOrder")`（MCP-08） | **通过**（并有机械断言钉住） |

## 2. 实测记录

### 2.1 指令字节数（约束 2）

~~~text
$ ./webctl mcp   # 发送 initialize（JSON-RPC over stdio）
  instructions 字节数 = 2222   (上限 32768)  -> OK
  serverInfo = {'name': 'webctl', 'version': '0.1.0'}
~~~

### 2.2 二次启动 / 并发启动（约束 3）

~~~text
$ ./webctl mcp   # 探测进程（起一次即退）
  探测进程 rc=0 有回复=True
$ ./webctl mcp   # 紧接的服务进程
  服务进程 rc=0 有回复=True
$ 两个并发 webctl mcp
  并发两个：A 有回复=True B 有回复=True
  判定 = 二次/并发启动均不冲突
~~~

**为什么这条重要**：DSH 会*先起一个探测进程、再起服务进程*；如果 MCP server 在启动时抢独占锁或写死单例文件，第二次启动就会失败，表现为"装了没反应、工具不出现"。webctl 不在启动时抢锁，因此两种启动都成立。

## 3. 长任务与 60s 预算

- 默认请求预算 `timeout_ms = 30_000`（可在配置/环境变量里调），**低于** DSH 的 60s 工具调用超时；
- 需要更多内容的场景按**分片**处理：`paginate`（页数上限）、`extract` 的结果/字节预算 + 截断标记 + `LIMIT` 码；
- 原则：工具**不**靠"跑很久直到超时"完成任务，超预算时返回带 `LIMIT`/`TIMEOUT` 码的信封而不是静默截断（见 PLAN §3 信封规则）。

## 4. "装了没反应"的排查顺序（约束 4）

1. `webctl mcp --list-tools` —— 确认 server 自身能起来并列出 15 个工具；
2. 手工发一次 `initialize`（§2.1 的命令）—— 确认 stdio 协商正常、无 stderr 噪音；
3. 检查 DSH 侧 MCP 配置的 server 名与命令路径是否与 README「安装」段一致；
4. 若 harness 侧连接失败，DSH 仍会启动但**工具不出现** —— 这是 DSH 的既定行为，不是 webctl 崩溃。

## 5. 未覆盖 / 待办（诚实登记）

- **未经真实 DSH 挂载的端到端验证**：本文全部结论来自 stdio 层探针与代码核对，**没有**在 DSH 里实际挂载后调用 `mcp__webctl__*`。该端到端验证仍是 [PR 17 / 议题 #28](https://github.com/Thatgfsj/webctl/issues/28) 的验收项，未完成前不得声称"DSH 集成已验收"。
- `toolCallTimeoutMs` 的**超时行为实测**（让一次调用故意跑满 60s 观察 harness 反应）未做。
