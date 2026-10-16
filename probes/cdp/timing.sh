#!/usr/bin/env bash
# 测真实时延：Edge 冷启动到 CDP 就绪、以及关闭耗时。跑 3 次取观察值。
set -uo pipefail
PORT_BASE=9400
EDGE="/usr/bin/microsoft-edge"

for i in 1 2 3; do
  PORT=$((PORT_BASE + i))
  PROFILE="$(mktemp -d /tmp/webctl-timing.XXXXXX)"
  T0=$(date +%s%N)
  "$EDGE" --headless=new --disable-gpu --no-first-run --no-default-browser-check \
    --remote-debugging-port="$PORT" --user-data-dir="$PROFILE" about:blank \
    >"$PROFILE/edge.log" 2>&1 &
  PID=$!
  for _ in $(seq 1 120); do
    curl -sf "http://127.0.0.1:$PORT/json/version" >/dev/null 2>&1 && break
    sleep 0.1
  done
  T1=$(date +%s%N)
  READY_MS=$(( (T1 - T0) / 1000000 ))
  kill "$PID" 2>/dev/null
  T2=$(date +%s%N)
  for _ in $(seq 1 50); do kill -0 "$PID" 2>/dev/null || break; sleep 0.1; done
  T3=$(date +%s%N)
  STOP_MS=$(( (T3 - T2) / 1000000 ))
  echo "run$i: CDP就绪=${READY_MS}ms  关闭=${STOP_MS}ms"
  kill -9 "$PID" 2>/dev/null
  rm -rf "$PROFILE"
done
