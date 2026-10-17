#!/usr/bin/env bash
# PR 行数口径的**唯一实现**（CONTRIBUTING §2 / PLAN §5.1）。
#
# 为什么要有这个脚本：CI 与人类必须用**同一套**排除规则，否则"文档说豁免、CI 判违规"
# （议题 #41 / #24 的核心诉求）。改排除规则时，本文件与 CONTRIBUTING §2 必须同改。
#
# 用法：
#   scripts/pr-size.sh <base-ref> [--limit N] [--allow-over]
#   scripts/pr-size.sh origin/main
#
# 输出：计入行数、各排除类别实际行数、结论；越线且未 --allow-over 时退出码 1。
# 依赖 bash 4+（关联数组）与 git；只在 CI（ubuntu）与本机开发时使用。
set -uo pipefail

BASE="${1:-origin/main}"
LIMIT=1000
ALLOW_OVER=0
shift || true
while [ $# -gt 0 ]; do
  case "$1" in
    --limit) LIMIT="$2"; shift 2 ;;
    --allow-over) ALLOW_OVER=1; shift ;;
    *) echo "未知参数：$1" >&2; exit 2 ;;
  esac
done

git rev-parse --verify --quiet "$BASE" >/dev/null || { echo "找不到 base ref：$BASE" >&2; exit 2; }

# 排除规则（与 CONTRIBUTING §2 逐条对应）
is_excluded() {
  case "$1" in
    docs/*) return 0 ;;                          # 纯文档
    crates/*/tests/fixtures/*) return 0 ;;       # 测试 fixture 数据文件
    Cargo.lock|*/Cargo.lock) return 0 ;;         # cargo 生成的锁文件（#41）
    .github/workflows/release.yml) return 0 ;;   # cargo-dist 生成（dist generate）
    CHANGELOG.md) return 0 ;;                    # 发布日志（发布流程写入条目）
    *) return 1 ;;
  esac
}

counted=0
declare -A bucket=()
declare -A files=()

while IFS=$'\t' read -r add del file; do
  [ -z "${file:-}" ] && continue
  case "$add" in '-') continue ;; esac        # 二进制文件：不计行数
  n=$(( add + del ))
  if is_excluded "$file"; then
    bucket["$file"]=$(( ${bucket["$file"]:-0} + n ))
    files["$file"]=1
  else
    counted=$(( counted + n ))
  fi
done < <(git diff --numstat "$BASE"...HEAD)

echo "== PR 行数口径（CONTRIBUTING §2）=="
echo "base            = $BASE"
echo "计入行数        = $counted"
echo "目标上限        = $LIMIT"
if [ "${#bucket[@]}" -gt 0 ]; then
  echo "不计入（但必须申报）："
  for f in "${!bucket[@]}"; do printf '  %-56s %s\n' "$f" "${bucket[$f]}"; done | sort
  excl_total=0; for f in "${!bucket[@]}"; do excl_total=$(( excl_total + ${bucket[$f]} )); done
  echo "不计入合计      = $excl_total"
else
  echo "不计入（但必须申报）：无"
fi
echo "shortstat       = $(git diff --shortstat "$BASE"...HEAD | sed 's/^ *//')"

if [ "$counted" -le "$LIMIT" ]; then
  echo "结论            = 未越线"
  exit 0
fi
if [ "$ALLOW_OVER" -eq 1 ]; then
  echo "结论            = 越线，但已放行（--allow-over：PR 带 lines: exempt 标签）"
  exit 0
fi
echo "结论            = 越线（$counted > $LIMIT）：需维护者在关联 issue 中显式批准，" >&2
echo "                  并在 PR 上打 lines: exempt 标签（CONTRIBUTING §2）。" >&2
exit 1
