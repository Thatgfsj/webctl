## 关联与认领

- 关联 issue：Closes #
- 是否已在 issue 下认领：是 / 否（未认领不通过）

## 变更类别（本 PR 只做哪一类）

- [ ] 取回（HTTP / 重定向链 / cookie / 编码 / 内容类型路由）
- [ ] 解析（DOM / 正文 / 链接 / 表格 / 表单 / markdown）
- [ ] 会话（填表提交 / CSRF / multipart / 分页 / 会话状态）
- [ ] 文档与数据（PDF 文本层 / json_query / 格式转换）
- [ ] 抽取与接口（extract / search / CLI / MCP 工具面）
- [ ] 安全与凭据（脱敏 / 文件权限 / 路径与协议校验）
- [ ] 测试基建 / CI
- [ ] 文档与治理

## 行数口径（CONTRIBUTING §2）

```text
$ git diff --shortstat <base>...HEAD
# additions + deletions =
```

不计入口径的文件实际行数（必须列明，无则写"无"）：

| 文件/目录 | 实际行数 | 类别 |
|---|---|---|
| | | 纯文档 / 测试 fixture |

- [ ] 数字已如实申报（用 `bash scripts/pr-size.sh origin/main` 生成，勿手填估算）
- [ ] 计入行数 ≤1000；若越线，已在关联 issue 获得维护者批准并在本 PR 打 `lines: exempt` 标签（CI 的 `pr-size` 作业会机械校验）

## 门禁命令执行结果

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test`
- [ ] 真站 smoke（`./tests/e2e/run-real.sh`，如涉及；需遵守目标站 robots）
- [ ] 体积基线（如涉及）
未执行项与原因：

> 测试必须**离线**：只连 `127.0.0.1` 上的本地 fixture/服务，不得访问外网。L0 离线 lane（`--features offline`）由 PR 13 落地，此前由各测试自身保证。

## 硬约束自查

- [ ] 零子进程（代码中无 `std::process` / `Command`）
- [ ] 无 `unsafe`
- [ ] 页面内容一律 `trust: untrusted_page_content`

## 降级与信任语义（如涉及）

- `meta.engine` / `code` / `meta.degraded_from` 的表现：
- 是否引入静默降级：否（本项目禁止静默降级）
