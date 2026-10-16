#!/usr/bin/env bash
# 发布资产 E2E 冒烟（TESTING PKG-04 / PKG-06 / PKG-08）。
#
# 在「干净环境」（无 Rust 工具链、无浏览器）里对已下载/解压的 webctl 二进制运行。
# 本脚本是开发/发布工具，允许调用外部命令；产品运行期零子进程约束不适用于它。
#
# 用法：
#   scripts/release-e2e.sh ./webctl [期望版本 X.Y.Z] [--static]
#
# --static 用于 musl 资产：ldd 必须显示静态；对 glibc 产物请省略。
# 退出码：0 全部通过；1 有失败项；2 用法错误。

set -uo pipefail

BIN=""
EXPECTED=""
REQUIRE_STATIC=0
for arg in "$@"; do
  case "$arg" in
    --static) REQUIRE_STATIC=1 ;;
    --*) echo "未知参数：$arg" >&2; exit 2 ;;
    *)
      if [ -z "$BIN" ]; then BIN="$arg"; elif [ -z "$EXPECTED" ]; then EXPECTED="$arg"; fi
      ;;
  esac
done

if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "用法：scripts/release-e2e.sh <webctl 二进制> [期望版本 X.Y.Z] [--static]" >&2
  exit 2
fi

FAIL=0

echo "== PKG-04 版本与 tag 一致 =="
VERSION_OUT="$("$BIN" --version 2>&1)"
echo "  $VERSION_OUT"
GOT="$(printf "%s" "$VERSION_OUT" | sed -n "s/.* \([0-9][0-9.]*\).*/\1/p")"
if [ -n "$EXPECTED" ]; then
  if [ "$GOT" = "$EXPECTED" ]; then
    echo "  OK: 版本 $GOT == tag $EXPECTED"
  else
    echo "  FAIL: 版本 $GOT != tag $EXPECTED"
    FAIL=$((FAIL + 1))
  fi
else
  echo "  SKIP：未提供期望版本，请用 tag 的 X.Y.Z 复跑"
fi

echo "== PKG-06 干净环境可运行：caps --json =="
CAPS="$("$BIN" caps --json 2>&1)"
if printf "%s" "$CAPS" | python3 -c "import json,sys
d=json.load(sys.stdin)
assert d['ok'] is True, d
data=d['data']
assert data['engine']=='builtin', data
doms=[c['domain'] for c in data['capabilities']]
assert len(doms)==5, doms
print('  OK: caps ok=true, engine=builtin, domains=' + ','.join(doms))
"; then
  :
else
  echo "  FAIL: caps --json 未通过校验"
  FAIL=$((FAIL + 1))
fi

echo "== PKG-08 无动态依赖（musl 产物应为静态） =="
LDD_OUT="$(ldd "$BIN" 2>&1 || true)"
if printf "%s" "$LDD_OUT" | grep -qiE "not a dynamic executable|statically linked"; then
  echo "  OK: 静态链接"
else
  printf "  动态依赖：%s\n" "$(printf "%s" "$LDD_OUT" | head -3 | tr "\n" " ")"
  if [ "$REQUIRE_STATIC" -eq 1 ]; then
    echo "  FAIL: --static 要求静态，但发现动态依赖"
    FAIL=$((FAIL + 1))
  else
    echo "  NOTE: glibc 产物动态属预期；musl 资产请加 --static 复跑"
  fi
fi

echo
if [ "$FAIL" -eq 0 ]; then
  echo "release-e2e: PASS"
  exit 0
fi
echo "release-e2e: FAIL（$FAIL 项）"
exit 1
