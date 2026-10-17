#!/usr/bin/env bash
# 安装并校验 x86_64-linux-musl 的 C++ 交叉工具链（CI / 本机共用）。
#
# 为什么需要它：wreq 的 BoringSSL（btls-sys）包含 C++ 源码，目标为 musl 时
# 必须有面向 musl 的 libstdc++。Ubuntu 的 musl-tools/musl-dev 只提供 C 侧，
# 因此需要自带 libstdc++ 的交叉工具链（议题 #90）。
#
# 选型：Bootlin stable 2026.08-1（Buildroot 构建）
#   - 组件：GCC 15.3.0 / binutils 2.45.1 / musl 1.2.6
#   - 来源：https://toolchains.bootlin.com/（Bootlin，知名嵌入式 Linux 厂商，
#     工具链由 Buildroot 构建；源码与 license 见 releases/sources 与 releases/licenses）
#   - 可信度：非 GCC/musl 官方发布物，但 HTTPS 分发 + 发布方提供 SHA256；
#     本脚本把 SHA256 硬编码在仓库里并每次校验，同时固定版本号，避免滚动更新。
#     缓存命中时也校验可执行文件与 libstdc++.a 存在，防止半损坏缓存。
#   - 体积：下载约 86 MiB，解压约 500 MiB；CI 用 actions/cache 缓存到 $HOME/.cache。
#
# 用法：
#   scripts/install-musl-toolchain.sh
# 成功时把工具链根目录打印到 stdout（日志走 stderr）；若检测到 GitHub Actions
# 环境，则同时写入 GITHUB_ENV / GITHUB_PATH，供后续 step 的 cargo/CMake 使用。

set -euo pipefail

VERSION="2026.08-1"
ARCHIVE="x86-64--musl--stable-${VERSION}.tar.xz"
EXPECTED_SHA256="78d3a4683d6ac47b5ee73bd5bce210b55eb93dff1b137c61298af97eb0d2b5a6"
EXPECTED_GCC="15.3.0"
URL="https://toolchains.bootlin.com/downloads/releases/toolchains/x86-64/tarballs/${ARCHIVE}"

CACHE="${MUSL_TOOLCHAIN_CACHE:-${HOME}/.cache/webctl-musl-toolchain}"
TC="${CACHE}/x86-64--musl--stable-${VERSION}"

log() { printf '%s\n' "$*" >&2; }
fail() { log "error: $*"; exit 1; }

[ "$(uname -s)" = "Linux" ] || fail "此脚本只支持 Linux（当前 $(uname -s)）"
command -v curl >/dev/null || fail "缺少 curl"
command -v sha256sum >/dev/null || fail "缺少 sha256sum"
command -v tar >/dev/null || fail "缺少 tar"

mkdir -p "${CACHE}"

if [ ! -x "${TC}/bin/x86_64-linux-g++" ]; then
  if [ ! -f "${CACHE}/${ARCHIVE}" ]; then
    log "==> 下载 Bootlin musl 工具链 ${VERSION}（约 86 MiB）"
    tmp="${CACHE}/${ARCHIVE}.part"
    rm -f "${tmp}"
    curl -fL --retry 3 --connect-timeout 20 -o "${tmp}" "${URL}"
    mv "${tmp}" "${CACHE}/${ARCHIVE}"
  fi

  log "==> 校验 SHA256"
  printf '%s  %s\n' "${EXPECTED_SHA256}" "${CACHE}/${ARCHIVE}" | sha256sum -c -

  log "==> 解压到 ${CACHE}"
  rm -rf "${TC}"
  tar -xJf "${CACHE}/${ARCHIVE}" -C "${CACHE}"
  rm -f "${CACHE}/${ARCHIVE}"
fi

[ -x "${TC}/bin/x86_64-linux-gcc" ] || fail "工具链缺少 x86_64-linux-gcc：${TC}"
[ -x "${TC}/bin/x86_64-linux-g++" ] || fail "工具链缺少 x86_64-linux-g++：${TC}"
[ -f "${TC}/x86_64-buildroot-linux-musl/sysroot/usr/lib/libstdc++.a" ] \
  || fail "工具链缺少面向 musl 的静态 libstdc++.a：${TC}"

GCC_OUT="$("${TC}/bin/x86_64-linux-g++" --version | head -n 1)"
printf '%s\n' "${GCC_OUT}" | grep -q "${EXPECTED_GCC}" \
  || fail "GCC 版本不符合预期（需要 ${EXPECTED_GCC}）：${GCC_OUT}"

log "==> 工具链就绪：${TC}"
log "    ${GCC_OUT}"

if [ -n "${GITHUB_ENV:-}" ]; then
  {
    printf 'MUSL_TOOLCHAIN=%s\n' "${TC}"
    printf 'CC_x86_64_unknown_linux_musl=%s\n' "${TC}/bin/x86_64-linux-gcc"
    printf 'CXX_x86_64_unknown_linux_musl=%s\n' "${TC}/bin/x86_64-linux-g++"
    printf 'CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=%s\n' "${TC}/bin/x86_64-linux-gcc"
    # bindgen 以 TARGET 调 clang，需要 musl sysroot 才能找到 stddef.h 等头文件；
    # 用 target 后缀，避免污染同一 runner 上的 gnu 构建。
    printf 'BORING_BSSL_SYSROOT_x86_64_unknown_linux_musl=%s\n' "${TC}/x86_64-buildroot-linux-musl/sysroot"
  } >> "${GITHUB_ENV}"
fi
if [ -n "${GITHUB_PATH:-}" ]; then
  printf '%s\n' "${TC}/bin" >> "${GITHUB_PATH}"
fi

printf '%s\n' "${TC}"
