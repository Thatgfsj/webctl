#!/usr/bin/env bash
# 启动一个隔离的 headless Edge 并跑手写 CDP 探针，结束后清理
set -uo pipefail

PORT="${1:-9333}"
HERE="$(cd "$(dirname "$0")" && pwd)"
PROFILE="$(mktemp -d /tmp/webctl-probe.XXXXXX)"
EDGE="$(command -v microsoft-edge || command -v microsoft-edge-stable || command -v google-chrome)"

echo "[probe] edge=$EDGE  port=$PORT  profile=$PROFILE"
"$EDGE" \
  --headless=new --disable-gpu --no-first-run --no-default-browser-check \
  --disable-blink-features=AutomationControlled \
  --remote-debugging-port="$PORT" --user-data-dir="$PROFILE" \
  about:blank >"$PROFILE/edge.log" 2>&1 &
EDGE_PID=$!

for _ in $(seq 1 40); do
  curl -sf "http://127.0.0.1:$PORT/json/version" >/dev/null 2>&1 && break
  sleep 0.5
done

if ! curl -sf "http://127.0.0.1:$PORT/json/version" >/dev/null 2>&1; then
  echo "[probe] CDP endpoint 未就绪，Edge 日志："
  tail -20 "$PROFILE/edge.log"
  kill "$EDGE_PID" 2>/dev/null
  exit 1
fi

node "$HERE/cdp-probe.mjs" "$PORT"
RC=$?

kill "$EDGE_PID" 2>/dev/null
sleep 1
kill -9 "$EDGE_PID" 2>/dev/null
rm -rf "$PROFILE"
exit $RC
