# 探针脚本（实测基线，可原地重跑）

本目录保存 [docs/SURVEY.md](../docs/SURVEY.md) 中实验的**原始脚本**，用于复现结论、并作为 T3 路线的可见基线。三个脚本都是 2026-10-16 在本机（Rust 1.98.1 / Edge 155.0.4283.45 / Node 24）真实跑过的版本，**未依赖 Playwright**。

| 文件 | 用途 | 对应证据 |
|---|---|---|
| `cdp/cdp-probe.mjs` | 手写 CDP 帧：连接、扁平会话、Page/Runtime/Network/DOM/Accessibility 启用、导航、真实点击、中文输入、a11y 树、网络事件、截图、移动仿真 | [SURVEY §2.2](../docs/SURVEY.md) |
| `cdp/run-cdp-probe.sh` | 启一个隔离 headless Edge（临时 `--user-data-dir`）→ 跑探针 → 清理进程与目录 | 同上 |
| `cdp/timing.sh` | Edge 冷启动到 CDP 就绪 / 关闭耗时，跑 3 次 | [SURVEY §2.3](../docs/SURVEY.md) |

## 运行

```bash
./probes/cdp/run-cdp-probe.sh 9333      # 需要 microsoft-edge / google-chrome 与 node
./probes/cdp/timing.sh
```

## 已观测结果（复现时应得到同样的量级）

- 协议 `1.3`，浏览器 `Edg/155.0.4283.45`；`Target.attachToTarget(flatten)` 成功。
- 点击走 `DOM.getBoxModel` 算坐标 + `Input.dispatchMouseEvent`；中文输入必须走 `Input.insertText`（`dispatchKeyEvent` 不可用）。
- `navigator.webdriver === false`；刷新后收到 18 条 `Network.*` 事件；截图约 7KB PNG。
- 冷启动到 CDP 就绪：305 / 292 / 313 ms；关闭：约 104 ms → 浏览器按会话懒启动可接受。
- **移动仿真陷阱**：只设 `Emulation.setDeviceMetricsOverride` 时 `window.innerWidth` 仍为 **980**（页面缺 `<meta name="viewport">`）。

## 纪律

- 这些脚本是**证据快照**，不是产品代码。产品化的测试入口在后续 PR 20（`tests/e2e/run-edge.sh`、mock CDP server）；届时从这里搬运并加断言，不要在探针上直接长出功能。
- 探针会真实启动浏览器并访问网络，请在受控环境重跑；不要把它接进默认 CI。
