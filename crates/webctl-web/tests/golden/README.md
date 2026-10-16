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
