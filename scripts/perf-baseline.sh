#!/usr/bin/env bash
# 解析/抽取性能基线（docs/TESTING.md §1 的 **L4 通道**；议题 #83）。
#
# 为什么需要它：单测的墙钟断言已被放宽成 30s「挂死守卫」（PARSE-14 / EXTRACT-14 /
# EXTRACT-17），于是 [~5s, 30s) 这一档**没有门禁**。绝对墙钟不是门禁的正确工具，
# 但"量级级退化"必须有人抓——本通道就是接手方。
#
# 门槛语义（诚实声明）：本通道用**相对基线 + 宽 tolerance**，抓的是**量级级退化**
# （例如从 ~200ms 掉到 ~13.9s），**不**用于抓 20% 级别的抖动；抖动由 tolerance 吸收。
#
# 用法：
#   scripts/perf-baseline.sh                # 跑探针并打印结果
#   scripts/perf-baseline.sh --record       # 把本次实测写回 docs/perf-baseline.json
#   scripts/perf-baseline.sh --check        # 与基线比较，超门槛则退出码 1
#   PERF_RUNS=11 scripts/perf-baseline.sh --check
#
# 本脚本是开发工具（允许调用 cargo / python3 / stat），产品运行期零子进程约束不适用。
set -uo pipefail

MODE="report"
for a in "$@"; do
  case "$a" in
    --record) MODE="record" ;;
    --check) MODE="check" ;;
    *) echo "未知参数：$a" >&2; exit 2 ;;
  esac
done

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BASELINE="$ROOT/docs/perf-baseline.json"
FACTOR="${PERF_FACTOR:-5}"      # 允许 5 倍以内（远小于量级级退化）
FLOOR_MS="${PERF_FLOOR_MS:-50}" # 小数值的绝对下限，避免噪声误判

cd "$ROOT"
echo "== 运行探针（cargo run --release --example perf_probe）=="
JSON="$(cargo run --quiet --release --example perf_probe -- --json 2>/dev/null | tail -1)"
if [ -z "$JSON" ] || ! printf '%s' "$JSON" | grep -q parse_100kb_ms_p50; then
  echo "探针未产出可解析的 JSON；原始输出：" >&2
  echo "$JSON" >&2
  exit 1
fi
echo "$JSON"

if [ "$MODE" = "record" ]; then
  TODAY="$(date +%F)"
  COMMIT="$(git rev-parse --short HEAD)"
  python3 - "$BASELINE" "$JSON" "$TODAY" "$COMMIT" <<'PY'
import json, sys
path, raw, day, commit = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
m = json.loads(raw)
doc = {
  "schema": 1,
  "description": "解析/抽取性能 p50 基线（TESTING §1 L4 / 议题 #83）。单位毫秒；由 scripts/perf-baseline.sh --record 写入。",
  "factor": 5,
  "floor_ms": 50,
  "metrics": {
    "parse_100kb_ms_p50": m["parse_100kb_ms_p50"],
    "extract_1mb_hit_ms_p50": m["extract_1mb_hit_ms_p50"],
    "extract_1mb_nohit_ms_p50": m["extract_1mb_nohit_ms_p50"],
    "extract_deep_ms_p50": m["extract_deep_ms_p50"],
  },
  "runs": m["runs"],
  "recorded_at": day,
  "commit": commit,
  "note": "相对基线 + 宽 tolerance：抓量级级退化，不抓 20% 抖动。",
}
open(path, "w", encoding="utf-8").write(json.dumps(doc, ensure_ascii=False, indent=2) + "\n")
print(f"已回填基线：{path}")
PY
  exit $?
fi

if [ "$MODE" = "check" ]; then
  if [ ! -f "$BASELINE" ]; then echo "缺少基线文件 $BASELINE（先 --record）" >&2; exit 1; fi
  python3 - "$BASELINE" "$JSON" "$FACTOR" "$FLOOR_MS" <<'PY'
import json, sys
path, raw, factor, floor = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4])
base = json.load(open(path, encoding="utf-8"))
m = json.loads(raw)
bad = []
for k, expect in base["metrics"].items():
  got = m[k]
  limit = max(expect * factor, floor)
  flag = "OK " if got <= limit else "FAIL"
  print(f"  {flag} {k:26s} 实测 {got:8.3f} ms  基线 {expect:8.3f}  上限 {limit:9.3f}")
  if got > limit: bad.append(k)
if bad:
  print(f"结论：量级级退化（{len(bad)} 项超门槛）: {', '.join(bad)}")
  sys.exit(1)
print("结论：全部在门槛内")
PY
  exit $?
fi
exit 0
