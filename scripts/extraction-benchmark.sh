#!/usr/bin/env bash
# 抽取质量基准（议题 #189）——开发工具，不在默认 CI 矩阵。
# 用法：scripts/extraction-benchmark.sh [--no-external]
#   BENCH_PROFILE=dev / REPORT_DIR=/tmp/bench / TRAFILATURA_PYTHON=... 可覆盖默认。
# 跑 webctl(markdown) + webctl(text) + 可选 trafilatura 对照（同一把尺子）；报告落 /tmp。
# 只读本地 fixtures、不联网；产品运行期的零子进程约束不适用。
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIXTURES="$ROOT/crates/webctl-web/tests/fixtures/extraction_bench"
OUT="${REPORT_DIR:-/tmp/webctl-extraction-bench}"
PROFILE="${BENCH_PROFILE:-release}"
RUN_EXTERNAL=1
for arg in "$@"; do
  case "$arg" in
    --no-external) RUN_EXTERNAL=0 ;;
    -h|--help) sed -n '2,5p' "$0"; exit 0 ;;
    *) echo "未知参数：$arg" >&2; exit 2 ;;
  esac
done
mkdir -p "$OUT"
cd "$ROOT" || exit 1
BENCH=(cargo run --quiet --profile "$PROFILE" --example extraction_bench --)
echo "commit=$(git rev-parse --short HEAD 2>/dev/null || echo unknown) profile=$PROFILE"
echo

run_bench() {
  echo "== $1 =="
  shift
  "${BENCH[@]}" "$@" || { echo "基准运行失败" >&2; exit 1; }
  echo
}

run_bench "webctl / parse::article（markdown 视图）" --fixtures "$FIXTURES" --report "$OUT/webctl.json"
run_bench "webctl / parse::article（text 视图，结构不保真）" --fixtures "$FIXTURES" --view text --report "$OUT/webctl-text.json"

TRAF_PYTHON="${TRAFILATURA_PYTHON:-python3}"
if [ "$RUN_EXTERNAL" = 1 ] && "$TRAF_PYTHON" -c 'import trafilatura' >/dev/null 2>&1; then
  PRED="$OUT/predictions-trafilatura"
  mkdir -p "$PRED"
  echo "== 生成 trafilatura 预测（output_format=txt，其余默认）=="
  "$TRAF_PYTHON" - "$FIXTURES" "$PRED" <<'PY' || { echo "trafilatura 预测生成失败" >&2; exit 1; }
import json, os, sys
import trafilatura
fixtures, out = sys.argv[1], sys.argv[2]
manifest = json.load(open(os.path.join(fixtures, "manifest.json"), encoding="utf-8"))
for sample in manifest["samples"]:
    html = open(os.path.join(fixtures, sample["html"]), encoding="utf-8").read()
    text = trafilatura.extract(html, output_format="txt") or ""
    with open(os.path.join(out, sample["id"] + ".txt"), "w", encoding="utf-8") as handle:
        handle.write(text + "\n")
    print("{:<12} lines={}".format(sample["id"], len(text.splitlines())))
PY
  echo
  run_bench "trafilatura（同一把尺子、同一批 fixtures）" --fixtures "$FIXTURES" --predictions "$PRED" --report "$OUT/trafilatura.json"
else
  echo "== trafilatura：不可用，跳过对照 =="
  echo
fi

echo "== 汇总（micro）=="
python3 - "$OUT" <<'PY'
import json, os, sys
out = sys.argv[1]
for name in ("webctl.json", "webctl-text.json", "trafilatura.json"):
    path = os.path.join(out, name)
    if not os.path.exists(path):
        continue
    report = json.load(open(path, encoding="utf-8"))
    t = report["totals"]
    print("{:<26} P={:.3f} R={:.3f} F1={:.3f}  ref={} pred={}".format(name, t["precision"], t["recall"], t["f1"], t["reference_paragraphs"], t["predicted_paragraphs"]))
    for c in report["categories"]:
        print("    {:<10} F1={:.3f}  n={} ref={} pred={}".format(c["category"], c["f1"], c["samples"], c["reference_paragraphs"], c["predicted_paragraphs"]))
print()
print("报告目录：" + out)
PY
