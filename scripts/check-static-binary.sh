#!/usr/bin/env bash
# 断言 Linux ELF 二进制是静态链接，并做最小运行冒烟（PKG-02 / PKG-08）。
#
# 用法：
#   scripts/check-static-binary.sh <二进制> [--no-smoke]
#
# 断言（任一失败即 exit 1）：
#   1. `file` 报告 statically linked / static-pie linked；
#   2. `ldd` 报告 not a dynamic executable / statically linked；
#   3. `readelf -d` 的动态段没有 NEEDED（不依赖共享库）；
#   4. 默认再跑 `--version` 与 `caps --json`（--no-smoke 可跳过）。
#
# 不要为了让构建通过而删掉断言：这三条共同证明「无动态依赖」。

set -euo pipefail

BIN="${1:-}"
SMOKE=1
for arg in "${@:2}"; do
  case "${arg}" in
    --no-smoke) SMOKE=0 ;;
    *) echo "未知参数：${arg}" >&2; exit 2 ;;
  esac
done

if [ -z "${BIN}" ] || [ ! -x "${BIN}" ]; then
  echo "用法：scripts/check-static-binary.sh <二进制> [--no-smoke]" >&2
  exit 2
fi

echo "== file =="
FILE_OUT="$(file -b "${BIN}")"
echo "  ${FILE_OUT}"
case "${FILE_OUT}" in
  *"statically linked"*|*"static-pie linked"*) ;;
  *) echo "FAIL: file 未报告静态链接" >&2; exit 1 ;;
esac

echo "== ldd =="
LDD_OUT="$(ldd "${BIN}" 2>&1 || true)"
echo "  ${LDD_OUT//$'\n'/$'\n  '}"
if ! printf '%s' "${LDD_OUT}" | grep -qiE 'not a dynamic executable|statically linked'; then
  echo "FAIL: ldd 显示动态依赖" >&2
  exit 1
fi

echo "== readelf -d（不得有 NEEDED） =="
if readelf -d "${BIN}" 2>/dev/null | grep -q '(NEEDED)'; then
  echo "FAIL: 动态段存在 NEEDED 条目" >&2
  exit 1
fi
echo "  OK: 无 NEEDED"

if [ "${SMOKE}" -eq 1 ]; then
  echo "== 运行冒烟 =="
  "${BIN}" --version
  "${BIN}" caps --json >/dev/null
  echo "  OK: --version 与 caps --json 可运行"
fi

echo "PASS: ${BIN} 为静态链接"
