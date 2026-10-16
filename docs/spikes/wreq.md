# wreq 验证报告（Spike #5）

| 项 | 值 |
|---|---|
| 议题 | [#5](https://github.com/Thatgfsj/webctl/issues/5)（[PLAN §9](https://github.com/Thatgfsj/webctl/blob/main/docs/PLAN.md) 待验证项 1） |
| 日期 | 2026-10-16 |
| 结论 | **可用**：wreq 0.16.1 在本机 Rust 1.98.1 上编译并成功发起真实 HTTPS 请求；TLS 指纹仿真**已实测生效**；代价是构建期需要 C/C++ 工具链 + libclang + clang 内建头 |
| 复现目录 | `/tmp/webctl-wreq-spike/`（探针工程、指纹抓取、依赖清单均在其中；本报告不依赖它长期存在） |

---

## 1. 结论先行

1. **编译**：`wreq 0.16.1` 默认特性即可，无需额外 feature；首次全量 release 构建 **76 秒**（16 核，含 BoringSSL 从源码编译）。
2. **MSRV**：crate 声明 `rust_version = 1.98`，与本机 `rustc 1.98.1` 一致，与 `rust-toolchain.toml` 的钉版一致。
3. **许可**：`wreq` / `wreq-util` / `wreq-proto` / `wreq-rt` 均为 Apache-2.0；可达依赖 133 个，全部为宽松许可（清单见 §5）。
4. **TLS 指纹**：`wreq-util 0.2.0` 的 `Emulation` 能真实改变 ClientHello —— 6 个抓取样本的 JA3/JA4/peetprint 哈希**互不相同**，且与 reqwest 明显不同（§6）。
5. **代价（必须一起记）**：默认 TLS 后端是 **BoringSSL**（经 `btls`/`btls-sys`），因此构建期需要 `cmake` + C/C++ 编译器 + **libclang**（bindgen）**+ clang 内建头文件**（`stddef.h` 等）。本机原本缺后两项，本次通过解包 Debian 的 `libclang1-19` / `libclang-common-19-dev` workaround 才编译成功（§4）。这是**构建期**依赖，不影响"运行时零外部进程"的约束。
6. 产物为**动态链接** libstdc++/libgcc（BoringSSL 的 C++ 代码所致）→ **musl 静态目标的交叉编译需要在 PR 14 单独验证**，不能想当然。

---

## 2. 环境

| 项 | 值 |
|---|---|
| 工具链 | `rustc 1.98.1 (48a229cea 2026-09-01)`，cargo 1.98.1 |
| 平台 | x86_64-unknown-linux-gnu，16 核 |
| 依赖源 | rsproxy-sparse 镜像（`~/.cargo/config.toml`） |
| 已有工具 | `cmake`（`~/.local/bin/cmake`）、`cc` / `c++` |
| **缺失工具** | `libclang.so`（系统无）、clang 内建头（`stddef.h`） |
| 网络 | 本机代理；`tls.peet.ws` 可达 |

---

## 3. 编译与体积

### 3.1 探针工程

```toml
# /tmp/webctl-wreq-spike/probe/Cargo.toml
[dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
wreq = "0.16.1"
wreq-util = "0.2.0"
```

程序（`src/main.rs`）：建 `wreq::Client` → `GET` → 打印状态、HTTP 版本、响应头、body 长度。

### 3.2 实测结果

| 项 | 值 |
|---|---|
| 解析版本 | `wreq 0.16.1`、`wreq-util 0.2.0` |
| 额外 feature | 无（默认特性即可） |
| 首次全量 release 构建 | **76 s**（`timed3`，含 BoringSSL 编译；第二次起走增量） |
| 真实请求 | ✅ `200 OK`，`HTTP/2.0`，`server: TrackMe.peet.ws` |
| crates.io 下载量 | 2,278,576（`wreq`）；crate 创建于 2026-08-27 |

### 3.3 体积（release、未 strip、同一工具链）

| 工程 | 依赖 | 二进制 |
|---|---|---|
| `baseline` | 无（hello world） | **457,960 B** |
| `control` | `reqwest 0.13`（rustls + http2 + webpki-roots）+ tokio | **8,166,856 B** |
| `probe` | `wreq 0.16.1`（默认，BoringSSL）+ `wreq-util` + tokio | **5,666,752 B** |

相对 hello world 的增量：wreq 路径 **≈ 5.21 MB**，reqwest 路径 **≈ 7.71 MB**。

> 说明：两者 TLS 后端不同（wreq 默认 BoringSSL / reqwest 此处 rustls），因此这不是"同后端比大小"的严格对比；结论只到"wreq 路径的体积没有失控，且不比 rustls 路径更大"。

---

## 4. 构建前置（本次踩到的坑，必须写进文档）

干净构建两次失败，第三次成功。失败与修复：

| 尝试 | 现象 | 原因 | 修复 |
|---|---|---|---|
| 1 | `bindgen: Unable to find libclang` | 系统无 `libclang.so` | 解包 Debian `libclang1-19`，设 `LIBCLANG_PATH=/tmp/webctl-wreq-spike/sysroot/libclang-lib` |
| 2 | `boring-sys failed: clang diagnosed error: 'stddef.h' file not found` | 缺 clang 内建头 | 解包 `libclang-common-19-dev`，设 `BINDGEN_EXTRA_CLANG_ARGS=-I…/usr/lib/llvm-19/lib/clang/19/include` |
| 3 | ✅ `Finished release profile in 1m 16s` | — | 上述两个环境变量 |

复现命令：

```bash
export LIBCLANG_PATH=/path/to/libclang          # 或安装 libclang-dev
export BINDGEN_EXTRA_CLANG_ARGS="-I/path/to/llvm-19/lib/clang/19/include"  # 或安装 clang
cargo build --release --locked
```

**含义**：在一台装了 `clang` + `libclang-dev` + `cmake` 的常规 Linux 上无需这些变量；但在"最小容器 / 干净 CI / 新开发机"上，这会成为第一道门槛。CI 与文档必须显式声明这些构建依赖（PR 14 处理）。

---

## 5. 依赖与许可

`cargo metadata` 可达包 **133** 个；许可直方图（全部为宽松许可，无强制 copyleft）：

| 许可 | 包数 |
|---|---|
| MIT OR Apache-2.0 | 50 |
| MIT | 22 |
| Unicode-3.0 | 18 |
| MIT/Apache-2.0 | 8 |
| Apache-2.0 | 7 |
| Apache-2.0 OR MIT | 7 |
| BSD-3-Clause | 5 |
| Unlicense OR MIT | 2 |
| Zlib | 2 |
| ISC / 0BSD / CDLA-Permissive-2.0 / BSD-3-Clause AND MIT / … | 各 1 |
| MIT OR Apache-2.0 OR LGPL-2.1-or-later | 1（OR 分支，可不选 LGPL） |

关键链路：

| crate | 版本 | 许可 | 备注 |
|---|---|---|---|
| `wreq` | 0.16.1 | Apache-2.0 | MSRV 1.98 |
| `wreq-util` | 0.2.0 | Apache-2.0 | 提供 `Emulation` 指纹档 |
| `wreq-proto` | 0.2.6 | Apache-2.0 | — |
| `wreq-rt` | 0.2.2-rc.4 | Apache-2.0 | **rc 版本**，需注意升级策略 |
| `btls` / `btls-sys` | 0.5.6 | Apache-2.0 / MIT | BoringSSL 绑定（构建期 C++） |
| `tokio` | 1.53.3 | MIT | MSRV 1.71 |

---

## 6. TLS 指纹证据（核心结论）

方法：同机同网络，对 `https://tls.peet.ws/api/all` 发请求，对比返回的 JA3 / JA4 / peetprint 哈希。抓取原始响应保存在 `/tmp/webctl-wreq-spike/fp/*.txt`。

| 客户端 | JA3 哈希 | JA4 | UA | ciphers / extensions |
|---|---|---|---|---|
| wreq 默认（plain） | `6aac88aefeb1c3f8b576782dbe085b6e` | `t13d2811h2_257f3020b3a2_78e6aca7449b` | 无 | 28 / 11 |
| wreq `Emulation::Chrome149` | `515c822f40054781f56b7097c4fe5e92` | `t13d1516h2_8daaf6152771_d8a2da3f94cd` | Chrome/149.0.0.0 (macOS) | 16 / 18（含 **GREASE**） |
| wreq `Emulation::Firefox149` | `6f7889b9fb1a62a9577e685c1fcfa919` | `t13d1717h2_5b57614c22b0_3cbfd9057e0d` | Firefox/149.0 | 17 / 17 |
| wreq `Emulation::Safari26` | `ecdf4f49dd59effc439639da29186671` | `t13d2013h2_a09f3c656075_7f0f34a4126d` | Safari (macOS) | 21 / 15 |
| wreq `Emulation::OkHttp5` | `f87c133faa73000b308cb1a1328b4ac0` | `t13d1613h2_46e7e9700bed_eca864cca44a` | NRC Audio（Android 14） | 16 / 13 |
| 对照组：`reqwest`（rustls） | `9c557c1a551bffba2f8ff016ac82a243` | `t13d1011h2_61a7ad8aa9b6_0d308c48d2a3` | 无 | 10 / 11 |

**判读**：

- 每个 `Emulation` 档位都得到**互不相同**的 JA3/JA4，说明指纹不是"改个 UA 字符串"的假仿真，而是真的改了 ClientHello（密文套件顺序、扩展集合、GREASE）。
- `plain`（wreq 默认）与 reqwest 也不同，说明 wreq 的默认栈本身就不是标准 rustls 指纹。
- 因此 **"TLS 指纹"这条选型理由成立**，具备可复核的证据。

---

## 7. 未验证 / 待办

| 项 | 状态 |
|---|---|
| `aws-lc` / `ring` 等非默认 TLS 后端组合 | 未验证（默认 BoringSSL 已够用） |
| macOS / Windows 构建 | 未验证（本机只有 Linux） |
| **musl 静态交叉编译**（`x86_64-unknown-linux-musl`） | 未验证，且因为 BoringSSL 的 C++ 链接**风险较高**，必须在 PR 14 专门处理 |
| `wreq-rt 0.2.2-rc.4` 这类 rc 依赖的升级策略 | 未定，需在 PR 02 决定钉版方式 |
| 长期维护节奏（wreq 更新频率、BoringSSL 跟进） | 未验证 |
| 与 Cloudflare 等真实拦截的对抗效果 | 不做承诺：本项目只提供指纹与合规姿态 |

---

## 8. 对计划的影响

1. **PLAN §3.2 选型确认**：T0 主 HTTP 客户端 = `wreq 0.16.1`（Apache-2.0，MSRV 1.98）+ `wreq-util 0.2.0`（指纹档）；`reqwest` 保留为可选回退。
2. **PLAN §9 待验证项 1 关闭**。
3. **新增构建前置要求**：`cmake` + C/C++ 编译器 + `libclang` + clang 内建头；须写入 README/CONTRIBUTING 的构建说明，并在 CI 中安装（PR 14）。
4. **新增风险项**：musl 静态 + BoringSSL 的交叉编译；产物当前动态链接 libstdc++/libgcc。
