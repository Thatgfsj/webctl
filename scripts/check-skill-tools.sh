#!/usr/bin/env bash
# MCP-12：断言仓库根 SKILL.md 声明的工具集合与 MCP tools/list 完全一致。
#
# 用法：
#   cargo build -p webctl            # 或先构建到自定义 target dir
#   scripts/check-skill-tools.sh
#   WEBCTL_BIN=/path/to/webctl scripts/check-skill-tools.sh
#
# 工具清单来自 `webctl mcp --list-tools`，与 tools/list 共用同一个注册表（TOOL_NAMES），
# 因此脚本不会出现“文档一份、代码一份”的漂移。
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="${WEBCTL_BIN:-$root/target/debug/webctl}"

if [[ ! -x "$bin" ]]; then
  echo "找不到可执行文件 $bin；先 cargo build -p webctl，或用 WEBCTL_BIN 指定" >&2
  exit 1
fi

declared="$(awk '/tools:start/ { inside=1; next } /tools:end/ { inside=0; next } inside && /^- [`][a-z_]+[`]/ { line=$0; sub(/^- [`]/, "", line); sub(/[`].*$/, "", line); print line }' "$root/SKILL.md" | LC_ALL=C sort)"
listed="$("$bin" mcp --list-tools | LC_ALL=C sort)"

if [[ "$declared" != "$listed" ]]; then
  echo "SKILL.md 与 tools/list 不一致：" >&2
  echo "--- tools/list ---" >&2
  printf "%s\n" "$listed" >&2
  echo "--- SKILL.md ---" >&2
  printf "%s\n" "$declared" >&2
  exit 1
fi

count="$(printf "%s\n" "$listed" | grep -c .)"
echo "SKILL.md 与 tools/list 一致（$count 个工具）"