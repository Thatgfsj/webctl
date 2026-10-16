## 关联与认领

- 关联 issue：Closes #
- 是否已在 issue 下认领：是 / 否（未认领不通过）

## 变更类别（本 PR 只做哪一类）

- [ ] 协议类型（`webctl-cdp-types`）
- [ ] CDP 底座（传输/分发/事件/target）
- [ ] T3 语义层（快照/交互/截图/网络）
- [ ] T0/T1（HTTP/解析/会话引擎）
- [ ] T2 文档桥
- [ ] MCP 工具面
- [ ] 测试基建 / CI
- [ ] 文档与治理

## 行数口径（CONTRIBUTING §2）

```
$ git diff --shortstat <base>...HEAD
# additions + deletions = 
```

不计入口径的文件实际行数（必须列明，无则写"无"）：

| 文件/目录 | 实际行数 | 类别 |
|---|---|---|
| | | 协议类型 / 纯文档 / fixture |

- [ ] 计入行数 ≤1000，或已在关联 issue 中取得显式批准（附批准链接）

## 门禁命令执行结果

- [ ] `cargo fmt --check`
- [ ] `cargo clippy -- -D warnings`
- [ ] `cargo test`
- [ ] 无浏览器 lane（如涉及）
- [ ] mock-cdp lane（如涉及）
- [ ] 真机 E2E（如涉及）
未执行项与原因：

## 降级与信任语义（如涉及）

- `meta.tier` / `code` / `meta.degraded_from` 的表现：
- 是否引入静默降级：否（本项目禁止静默降级）
