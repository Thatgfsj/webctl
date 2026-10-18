#!/usr/bin/env bash
# 例2：capture 显式关闭 -> 不应产生任何 .dsh-ops 写入
# 例3：cwd 位于 DSH home 之内 -> 应被拒绝，不落盘
set -u
ROOT="/home/thatgfsj/AIagent/webctl/ops032测试"
PATCH="$ROOT/工具测试/e2e/panel-off.patch.yml"

echo "########## CASE 2: capture=false ##########"
cd "$ROOT/工具测试/e2e/capture-off" || exit 1
rm -rf .dsh-ops
find . -print0 | sort -z | xargs -0 stat -c '%n %s' > before.txt
dsh --profile ops-panel-e2e --patch "$PATCH" --json "只回复 OFF-OK" > run.jsonl 2> run.err
echo "E2 rc=$?"
echo "--- E2 stderr ---"; cat run.err
echo "--- E2 新增/变更文件 ---"; find . -print0 | sort -z | xargs -0 stat -c '%n %s' > after.txt; diff before.txt after.txt || true
echo "--- E2 是否存在 .dsh-ops ---"; find . -name '.dsh-ops' -print | sed 's/^/FOUND: /'; echo "(以上为空即无写入)"

echo
echo "########## CASE 3: cwd 在 DSH home 之内 ##########"
cd "$HOME/.dsh/panel-probe-cwd" || exit 1
rm -rf .dsh-ops
dsh --profile ops-panel-e2e --json "只回复 HOME-OK" > run.jsonl 2> run.err
echo "E3 rc=$?"
echo "--- E3 stderr ---"; cat run.err
echo "--- E3 目录内容 ---"; ls -la
echo "--- E3 是否存在 .dsh-ops ---"; find . -name '.dsh-ops' -print | sed 's/^/FOUND: /'; echo "(以上为空即被拒绝)"
echo "--- 最近的 DSH 启动日志中的 panel 相关行 ---"
LOG=$(ls -t "$HOME/.dsh/logs"/*.log 2>/dev/null | head -1)
echo "log=$LOG"
grep -i -n 'panel\|\.dsh-ops' "$LOG" 2>/dev/null | tail -10
