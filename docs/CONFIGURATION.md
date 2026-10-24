# 配置与 CLI（webctl v0.4）

本文件对应 PLAN v0.4 §3.3/§3.7 与 TESTING CLI-01…CLI-12。
CLI 与（后续的）MCP 共享同一套配置语义：合并规则在 `webctl-core` 的 config 模块，
CLI 只负责参数解析与输出。

## 1. 四层优先级

**CLI 参数 > 环境变量 > 配置文件 > 内置默认**（高 → 低）。
每层只覆盖自己显式给出的键，未给出的键继续沿用低优先级的值。

| 层 | 来源 |
|---|---|
| CLI 参数 | `--output` / `--json` / `--offline` / `--proxy` / `--timeout-ms` / `--config` |
| 环境变量 | 见 §3 表格 |
| 配置文件 | TOML，见 §2 |
| 内置默认 | `output=json`、`timeout_ms=30000`、`offline=false`、无代理 |

环境变量取值非法（例如 `WEBCTL_OUTPUT=yaml`）会明确报错并按 `PARSE`（退出码 6）退出，
不会被静默忽略，也不会被更高优先级的 CLI 参数顺带掩盖。

## 2. 配置文件

路径按以下顺序取第一个可用值：

1. `--config <PATH>`（显式给出但文件不存在 → 报错退出，不静默回落）
2. `$WEBCTL_CONFIG`
3. `$XDG_CONFIG_HOME/webctl/config.toml`
4. `$HOME/.config/webctl/config.toml`（Windows 用 `$USERPROFILE`）
5. `./webctl.toml`

默认路径下的文件不存在视为“没有配置文件”（使用默认值）；显式路径不存在则报错。

格式为 TOML，目前可用键：

```toml
output = "json"       # json | text
timeout_ms = 30000    # 单次请求预算（毫秒）
offline = false       # true 时拒绝一切网络访问
proxy = "http://127.0.0.1:8080"  # 显式代理；不写即直连
js_render = false     # 是否执行页面内联脚本；默认 false（ADR-0005）
```

语法错误或未知键会报“配置文件 <path> 解析失败：…”并以非零退出码结束；
未知键不会被忽略（拼错键会立即暴露）。

## 3. 环境变量

只有下表列出的变量会被读取；未列出的环境变量（尤其是宿主 `HTTP_PROXY` / `HTTPS_PROXY` /
`ALL_PROXY`）**一律不影响 webctl**。

| 变量 | 含义 | 取值 |
|---|---|---|
| `WEBCTL_CONFIG` | 配置文件路径 | 路径 |
| `WEBCTL_STATE_DIR` | 状态目录（仅路径；锁/TTL 属议题 #34） | 路径 |
| `WEBCTL_OUTPUT` | 输出格式 | `json` / `text` |
| `WEBCTL_TIMEOUT_MS` | 单次请求预算 | 非负整数（毫秒） |
| `WEBCTL_OFFLINE` | 离线模式 | `1/true/yes/on` 或 `0/false/no/off` |
| `WEBCTL_PROXY` | 显式代理 URL | 例 `http://user:pass@host:8080` |
| `WEBCTL_JS_RENDER` | 是否执行页面内联脚本（**默认关闭**） | `1/true/yes/on` 或 `0/false/no/off` |
| `XDG_CONFIG_HOME` | 配置文件目录基准 | 路径 |
| `HOME` | 配置文件/状态目录基准 | 路径 |
| `USERPROFILE` | Windows 上的 `HOME` 等价物 | 路径 |

变量名大小写不敏感（同时尝试全大写与小写形式）。

## 4. 代理语义（CLI-03）

- 只有**显式配置**才走代理：`WEBCTL_PROXY`、配置文件 `proxy`、CLI `--proxy`。

- 未配置时固定直连：取回层在 `proxy = None` 时显式 `no_proxy()`，
  **不会**读取宿主 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`。
  这样离线测试与 CI 不会被开发机上的代理劫持。
- 代理 URL 中的 `user:password@` 凭据在**任何输出**里都以 `***` 掩码；
  响应中的 `Set-Cookie` / `Authorization` 等敏感头在 CLI 输出里替换为 `<redacted>`。

## 5. 引擎开关（`js_render`，默认关闭，见 [ADR-0005](adr/0005-embedded-js-engine.md)）

- 默认 **不执行**任何页面脚本；开启是显式的（配置文件 `js_render` 或 `WEBCTL_JS_RENDER`）。
- **开关 ≠ 已接线**：`caps --json` 的 `data.js` 分开报告 `enabled`（调用方是否开启）与 `wired`（引擎是否真的接上了取回/抽取路径），并给出 `reason`。
  这样不会出现"配置说开着、实际没执行"的假象。
- **已接线的范围（`wired: true`）**：`resolve_source`（所有以 URL 为输入的解析类工具）与 `extract` 的 URL 分支；
  条件：**开关开启 且 内容是 HTML**。
- **只执行内联脚本**：外链 `<script src>` **不取**（避免页面内容驱动网络请求）——因此依赖 jQuery 等外部库的页面会**如实降级**
  （`js-render-failed`，附 `ReferenceError: $ is not defined` 这类原始证据），而不是"成功但空"。
- 引擎是**进程内**的 `crates/webctl-js`（纯 Rust，`boa_engine`）：不启动外部进程、不碰网络与文件系统；
  限值只有 loop / recursion / stack 与输入预算，**墙钟与堆内存不设限**（见 [#130](https://github.com/Thatgfsj/webctl/issues/130)）。

## 6. 输出格式与退出码

`--output json`（默认；`--json` 是等价别名，且显式给出时覆盖 `--output`）：
单行紧凑 JSON 结果信封，可直接管道给下游解析器（CLI-09）。
`--output text`：人类可读文本；失败信息写到 stderr。

错误码与进程退出码是稳定映射（一经发布不得更改）：

| 错误码 | 退出码 |
|---|---|
| `OK` | 0 |
| `INTERNAL` | 1 |
| `BAD_URL` | 2 |
| `TIMEOUT` | 3 |
| `NETWORK` | 4 |
| `BLOCKED` | 5 |
| `PARSE` | 6 |
| `DEGRADED` | 7 |
| `UNSUPPORTED` | 8 |
| `SESSION_LOST` | 9 |
| `LIMIT` | 10 |

参数解析失败（未知参数、缺少必填参数）由 clap 以退出码 2 结束，并给出可读提示。
`--help` / `--version` 退出码为 0。

## 7. 子命令

| 子命令 | 说明 |
|---|---|
| `caps` | 内置能力自检（不联网、不探测外部程序）；含 **SPA 支持面**（认得的容器类型 + 真实可用的替代通道，0.1.2 / #115） |
| `fetch <URL>` | 取回 http(s) URL，输出结果信封 |
| `config` | 打印生效配置（代理凭据已脱敏） |

`fetch` 是配置面（代理 / 离线 / 超时）的最小可验证载体：
它在发出请求前检查 `offline`，并把解析后的代理与超时交给取回层。

## 8. 越界说明

本文件描述的范围是 PR 10 落地的配置与 CLI 行为。会话状态持久化（state dir 文件锁、
TTL、损坏恢复）属议题 #34；能力开关、MCP 工具面分别属其他议题。

## 浏览器上下文注入（0.2 M0 / ADR-0006）

宿主（agent/LLM）可从**设备浏览器**读取上下文并注入给 webctl，用于"携带用户会话"的取回。webctl **不读取、不驱动浏览器**。

| 键 | 形式 | 说明 |
|---|---|---|
| `WEBCTL_BROWSER_CONTEXT` | 环境变量，值为**JSON 文件路径** | 指向 `{ user_agent, accept, accept_language, sec_ch_ua, referer, cookies[] }`；未设置 = 不注入 |

### 安全与诚实边界（必须知道）

- **凭据类头（`cookie` / `referer`）只在同源请求携带**；跨源重定向一律丢弃（与 SEC-06 一致）；
- **cookie 值绝不落盘、绝不落日志、绝不进 envelope**：`BrowserContext` **刻意不实现 `Serialize`**（有编译期用例钉住），`Debug` 只输出条数；
- 注入前按优先级去重：jar 实际发出的 Cookie / 显式 `user_agent` / 调用方 `headers`  >  上下文（避免重复 Cookie 头）；
- **空 jar 不压制上下文 cookie**（有 `t6` 回归用例）；
- **诚实边界**：只借用 HTTP 头与 Cookie；**TLS 指纹仍是 wreq 的**、JS/WebGL/字体指纹**不存在** ✗；**不保证**绕过任何反爬；被拦即 `BLOCKED`；
- 解析失败会向 stderr 打印原因（**不静默**），并按"不注入"处理。

> `--browser-context <file>` CLI 开关尚未落地 ✗（当前入口是环境变量）；见议题 #158。