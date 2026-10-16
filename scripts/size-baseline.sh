#!/usr/bin/env bash
# webctl 体积与冷启动基线（TESTING PKG-05 / PKG-08 / CLI-12）。
#
# 本脚本属于开发工具链：允许调用 cargo / stat / date / ldd / python3。
# 产品运行期的「零子进程」硬约束（PLAN v0.4 §3.1）不适用于本脚本。
#
# 用法：
#   scripts/size-baseline.sh                                   # 构建 host release 并测量
#   scripts/size-baseline.sh --target x86_64-unknown-linux-musl
#   scripts/size-baseline.sh --binary ./webctl --target x86_64-unknown-linux-musl
#   scripts/size-baseline.sh --check                            # 与 docs/size-baseline.json 比较
#   scripts/size-baseline.sh --record                           # 把本次实测写回基线文件
#   scripts/size-baseline.sh --json                             # 机器可读输出
#
# 基线文件：docs/size-baseline.json；bytes=null 表示「待 CI 回填」。
# 体积容差 10%，冷启动（--version）上限 50 ms（CLI-12）。

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASELINE="$ROOT/docs/size-baseline.json"
TOLERANCE_PCT=10
COLD_START_LIMIT_MS=50
RUNS=10
TARGET=""
BINARY=""
DO_CHECK=0
DO_RECORD=0
AS_JSON=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --target) TARGET="$2"; shift 2 ;;
    --binary) BINARY="$2"; shift 2 ;;
    --runs) RUNS="$2"; shift 2 ;;
    --check) DO_CHECK=1; shift ;;
    --record) DO_RECORD=1; shift ;;
    --json) AS_JSON=1; shift ;;
    -h|--help) sed -n "2,20p" "$0"; exit 0 ;;
    *) echo "未知参数：$1" >&2; exit 2 ;;
  esac
done

host_triple() { rustc -vV | sed -n "s/^host: //p"; }
[ -n "$TARGET" ] || TARGET="$(host_triple)"

size_bytes() {
  case "$(uname -s)" in
    Darwin) stat -f%z "$1" ;;
    *) stat -c%s "$1" ;;
  esac
}

now_ms() {
  local raw
  raw="$(date +%s%N 2>/dev/null || true)"
  if [ "${#raw}" -ge 16 ]; then
    echo $((raw / 1000000))
  else
    python3 -c "import time; print(int(time.time()*1000))"
  fi
}

if [ -z "$BINARY" ]; then
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
  echo "==> 构建 release：target=$TARGET" >&2
  cargo build --release --target "$TARGET" >&2
  BINARY="$CARGO_TARGET_DIR/$TARGET/release/webctl"
  [ -x "$BINARY" ] || BINARY="$CARGO_TARGET_DIR/$TARGET/release/webctl.exe"
fi
if [ ! -x "$BINARY" ]; then
  echo "找不到可执行文件：$BINARY" >&2
  exit 2
fi

BYTES="$(size_bytes "$BINARY")"

cold_start_stats() {
  local bin="$1" runs="$2" i start end
  local times=()
  for ((i = 0; i < runs; i++)); do
    start="$(now_ms)"
    "$bin" --version >/dev/null 2>&1 || true
    end="$(now_ms)"
    times+=( "$((end - start))" )
  done
  printf "%s\\n" "${times[@]}" | sort -n > /tmp/webctl-cold-start-$$.txt
  COLD_MIN="$(head -1 /tmp/webctl-cold-start-$$.txt)"
  COLD_MAX="$(tail -1 /tmp/webctl-cold-start-$$.txt)"
  COLD_P50="$(sed -n "$(( (runs + 1) / 2 ))p" /tmp/webctl-cold-start-$$.txt)"
  rm -f /tmp/webctl-cold-start-$$.txt
}

COLD_P50=0
COLD_MIN=0
COLD_MAX=0
cold_start_stats "$BINARY" "$RUNS"

LINKAGE="dynamic"
if ldd "$BINARY" 2>&1 | grep -qiE "not a dynamic executable|statically linked"; then
  LINKAGE="static"
fi

baseline_field() {
  python3 -c "import json,sys; d=json.load(open(sys.argv[1],encoding='utf-8')); e=d.get('entries',{}).get(sys.argv[2],{}); v=e.get(sys.argv[3]); print('' if v is None else v)" "$BASELINE" "$TARGET" "$1"
}

BASELINE_BYTES="$(baseline_field bytes)"
LIMIT=""
if [ -n "$BASELINE_BYTES" ]; then
  LIMIT=$(( BASELINE_BYTES + BASELINE_BYTES * TOLERANCE_PCT / 100 ))
fi

STATUS="ok"
if [ -n "$LIMIT" ] && [ "$BYTES" -gt "$LIMIT" ]; then
  STATUS="size-regression"
fi
if [ "$COLD_P50" -gt "$COLD_START_LIMIT_MS" ]; then
  STATUS="cold-start-regression"
fi

if [ "$DO_RECORD" -eq 1 ]; then
  TODAY="$(date +%Y-%m-%d)"
  COMMIT="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  python3 -c "import json,sys
p=sys.argv[1]
d=json.load(open(p,encoding='utf-8'))
e=d.setdefault('entries',{}).setdefault(sys.argv[2],{})
e.update({'bytes':int(sys.argv[3]),'cold_start_ms_p50':int(sys.argv[4]),'linkage':sys.argv[5],'source':'local','recorded_at':sys.argv[6],'commit':sys.argv[7]})
with open(p,'w',encoding='utf-8') as f:
    json.dump(d,f,ensure_ascii=False,indent=2)
    f.write(chr(10))
" "$BASELINE" "$TARGET" "$BYTES" "$COLD_P50" "$LINKAGE" "$TODAY" "$COMMIT"
  echo "==> 已回填基线：$BASELINE（$TARGET）" >&2
fi

if [ "$AS_JSON" -eq 1 ]; then
  printf '{"target":"%s","binary":"%s","bytes":%s,"cold_start_ms_p50":%s,"cold_start_ms_min":%s,"cold_start_ms_max":%s,"linkage":"%s","baseline_bytes":%s,"limit_bytes":%s,"status":"%s"}\n' \
    "$TARGET" "$BINARY" "$BYTES" "$COLD_P50" "$COLD_MIN" "$COLD_MAX" "$LINKAGE" "${BASELINE_BYTES:-null}" "${LIMIT:-null}" "$STATUS"
else
  printf 'target       = %s\n' "$TARGET"
  printf 'binary       = %s\n' "$BINARY"
  printf 'bytes        = %s\n' "$BYTES"
  printf 'cold start   = p50 %s ms / min %s / max %s (runs=%s)\n' "$COLD_P50" "$COLD_MIN" "$COLD_MAX" "$RUNS"
  printf 'linkage      = %s\n' "$LINKAGE"
  if [ -n "$BASELINE_BYTES" ]; then
    printf 'baseline     = %s bytes (limit %s, +%s%%)\n' "$BASELINE_BYTES" "$LIMIT" "$TOLERANCE_PCT"
  else
    printf 'baseline     = 待 CI 回填（musl 数字待首个 release 产物）\n'
  fi
  printf 'status       = %s\n' "$STATUS"
fi

if [ "$DO_CHECK" -eq 1 ] && [ "$STATUS" != "ok" ]; then
  exit 1
fi
exit 0
