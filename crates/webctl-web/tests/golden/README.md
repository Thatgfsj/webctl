# golden 基线约定

本目录存放**信封 schema 快照**（后续 PR 可扩展解析/抽取期望输出）。
目的：字段名、类型、枚举取值发生有意或无意变化时，测试必须失败并强制人工确认。

## 比对

默认的 `cargo test` 与 L0 通道 `cargo test --features offline` 都只**比对**，不写文件：

    cargo test --features offline --test golden_envelope

不匹配时测试给出实际与期望的逐字 diff，并提示更新命令。

## 有意更新基线

1. 确认这是**有意**的协议/输出变更，而不是顺手改测试；
2. 运行更新命令（示例）：

       UPDATE_GOLDEN=1 cargo test --features offline --test golden_envelope

3. `git diff` **人工审阅** tests/golden/ 下的改动，确认字段变化符合预期；
4. 把 golden 变更与实现变更放进同一个 PR，并在 PR 描述里说明"有意更新基线"的理由。

> 更新模式只写基线文件，不被 CI 使用。写入前对文本做 LF 归一化并在结尾补换行，
> 避免不同平台的换行差异造成假失败。

## 离线扫描规则（offline lane）

`cargo test --features offline` 额外执行两条静态断言。规则权威实现见
`tests/support/resources.rs`（本节与其同步），并由
`resource_observation.rs::scanners_detect_planted_violations` 自证"植入违规必然被检出"。

### 外站 URL（`external_url_sites`）

- 范围：`crates/` 下路径含 `/tests/` 的 `.rs`；
- 先剥离注释（`//`、`///`、`//!`、`/* … */`，保留行号与字符串字面量），注释里的 URL 不算；
- 同时检查 `http://` 与 `https://`（不再"见 https 即违规"）；
- scheme 之后的 authority 去掉 `{…}` 占位符后为空 → **模板主机**，放行：
  `format!("http://{addr}/")`、`format!("http://{}", self.addr)`；
- 回环地址（`127.0.0.0/8`、`localhost`、`::1`）→ 放行；
- RFC 6761/2606 保留域（`.test`、`.invalid`、`.example`、`.localhost`、
  `example.com/net/org`）→ 放行：规范上不可解析，只用于 `Url::parse`/cookie
  等纯本地构造；
- 其余 authority → 判为外站字面量，断言失败；
- 扫描器自身文件按定义排除（见 `SCANNER_SELF_SUFFIX`）。

### 子进程调用点（`subprocess_call_sites`）

- 范围：`crates/` 下全部 `.rs`（含 `src/`）；
- 先剥离注释，避免文档注释里的字面量自命中；
- 匹配 `Command::new(` 与 `.spawn(`（真正的调用点）；`use std::process::Command;` 这类纯导入不算违规（见下方 2026-10-16 修订）；
- 扫描器自身文件排除。

### 2026-10-16 修订（修两处真实误报）

`main` 合并 PR 13 后门禁变红，根因是两条规则把**合规代码**判成违规，已修正：

- **外站 URL**：①scheme **大小写不敏感**（`HTTPS://` 同样是外站，不能当绕过手段）；
  ②authority **先剥 userinfo 再判定**——`http://user:pass@127.0.0.1:1` 是回环而非外站
  （`user:pass@` 里的 `:` 不是 authority 分隔符）；③IPv6 字面量 `[::1]` 整体取出
  （此前会被内层 `:` 截断成 `[`）。
- **子进程调用点**：①只匹配**真正的调用点**（`Command::new(` / `.spawn(`）；
  `use std::process::Command;` 这类纯导入不再算违规（导入本身不创建进程）；
  ②**放行"启动本仓自己的二进制"**——调用点自身或紧邻前两行出现 `CARGO_BIN_EXE_` 时跳过。
  集成测试用 `Command::new(env!("CARGO_BIN_EXE_webctl"))` 跑自己的 CLI 是标准做法，
  不是"外部进程"；产品源码（`src/`）里的调用点照常报违规。
- **成对护栏**：`scanners_detect_planted_violations`（该报的必报）与
  `scanner_does_not_flag_own_binary_or_loopback_userinfo`（该放行的不误报）。
- **已修**：IPv4 回环不再用 `starts_with("127.")` 前缀匹配（那会放行 `127.0.0.1.evil.com` 这类真域名），
  改为解析为 `Ipv4Addr` 后判 `is_loopback()`。
- 已知残余绕过（当前仓库不出现，记录在案）：`use std::process::{Command as C}; C::new(` 这类
  别名/分组导入、以及 `::` 之间插入空格，字面匹配抓不到。

### 目录缺失即失败

两个扫描器在根目录不存在/不可读时返回 `Err`（测试里 `expect`），**绝不**静默返回空集：
路径写错必须变成测试失败，而不是假绿。
