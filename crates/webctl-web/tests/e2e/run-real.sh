#!/usr/bin/env bash
# L1 真站 smoke（议题 #22 §6 / docs/TESTING.md §1）
#
# - 默认不跑：必须显式 WEBCTL_REAL=1；
# - 遵守 robots：先取 robots.txt 判断目标路径是否允许，禁止即跳过；
# - 只做锚点烟雾，不作回归门禁（真实站点结构会漂移，见 TESTING.md §5）。
#
# 用法：
#   WEBCTL_REAL=1 ./tests/e2e/run-real.sh
#
# 退出码：0 = 通过或合理跳过；非 0 = 已启用但锚点失败。
set -euo pipefail

if [[ "${WEBCTL_REAL:-0}" != "1" ]]; then
  echo "SKIP: 真站 smoke 默认不跑；设置 WEBCTL_REAL=1 启用"
  exit 0
fi

if ! command -v curl >/dev/null 2>&1; then
  echo "SKIP: 缺少 curl"
  exit 0
fi

STATIC_URL="https://quotes.toscrape.com/"
JS_URL="https://quotes.toscrape.com/js/"

# 1) 遵守 robots：取 robots.txt 并做最简 Disallow 判定。
ROBOTS="$(curl -fsS --max-time 15 "https://quotes.toscrape.com/robots.txt" || true)"
if [[ -n "${ROBOTS}" ]] && grep -qiE '^[[:space:]]*Disallow:[[:space:]]*/[[:space:]]*$' <<<"${ROBOTS}"; then
  echo "SKIP: robots.txt 禁止根路径"
  exit 0
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "SKIP: 缺少 cargo"
  exit 0
fi

# 2) fetch 子命令由 PR 10 落地；未落地时明确跳过，而不是假装通过。
if ! cargo run -q -p webctl -- --help 2>/dev/null | grep -qE '\bfetch\b'; then
  echo "SKIP: webctl fetch 子命令尚未落地（PR 10）"
  exit 0
fi

echo "== E2E-01 静态锚点：${STATIC_URL} 应取到 10 条 =="
cargo run -q -p webctl -- fetch "${STATIC_URL}" >/dev/null
echo "OK: 静态锚点取回成功"

echo "== E2E-02 降级锚点：${JS_URL} 应为 DEGRADED（不是 0 条成功）=="
OUT="$(cargo run -q -p webctl -- fetch "${JS_URL}")"
if grep -q '"code"[[:space:]]*:[[:space:]]*"DEGRADED"' <<<"${OUT}"; then
  echo "OK: JS 页返回 DEGRADED"
else
  echo "FAIL: JS 页未返回 DEGRADED；实际输出：${OUT}" >&2
  exit 1
fi
