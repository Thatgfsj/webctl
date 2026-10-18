#!/usr/bin/env bash
# 依赖边界守卫（ADR-0005）：
#   - 禁止外部浏览器 / CDP / 重量级 JS 运行时（V8、deno_core、mozjs…）；
#   - 允许且仅允许一个纯 Rust 内置 JS 引擎：boa_engine；
#   - 运行时零外部进程的相邻约束（Command/spawn）由测试与 hard-constraints 审计覆盖，这里只管依赖名。
#
# 用法：scripts/check-dependency-boundary.sh [Cargo.lock]
set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
lock="${1:-$root/Cargo.lock}"
[ -f "$lock" ] || { echo "找不到 Cargo.lock：$lock" >&2; exit 2; }

fail=0

# 1) 禁止清单（逐个精确匹配包名）
DENY="v8 deno_core deno_ast mozjs spidermonkey duktape quickjs rquickjs javascriptcore playwright chromium selenium chromedriver webdriver cdp"
for name in $DENY; do
  if grep -qx "name = \"$name\"" "$lock"; then
    echo "::error file=Cargo.lock,title=依赖边界::禁止的依赖：$name（见 docs/adr/0005-*.md）"
    fail=1
  fi
done

# 2) 允许清单：唯一允许的 JS 引擎是 boa_engine（纯 Rust）
if grep -qx "name = \"boa_engine\"" "$lock"; then
  echo "允许的 JS 引擎：boa_engine（纯 Rust，受限运行，ADR-0005）"
else
  echo "未引入 boa_engine（也可以：默认关闭的渲染能力，见 #128）"
fi

# 2b) 归属校验：boa_engine **只允许**被 webctl-js 依赖（ADR-0005 / 议题 #127）。
#
# 用 `cargo metadata` 的依赖图判定，而不是 grep Cargo.toml：
# grep 会被 TOML 书写形式（缩进、[dependencies.boa_engine] 子表）与 package 重命名绕过。
if grep -qx "name = \"boa_engine\"" "$lock"; then
  if ! command -v cargo >/dev/null 2>&1; then
    echo "::error file=Cargo.toml,title=依赖边界::需要 cargo 才能校验 boa_engine 归属" 
    fail=1
  else
    offenders="$(cargo metadata --locked --no-deps --format-version 1 2>/dev/null | python3 -c '
import json, sys
try:
    data = json.load(sys.stdin)
except Exception:
    print("__metadata_failed__")
    raise SystemExit(0)
bad = [
    package["name"]
    for package in data["packages"]
    if package["name"] != "webctl-js"
    and any(dep["name"] == "boa_engine" for dep in package.get("dependencies", []))
]
print(" ".join(sorted(bad)))
')"
    if [ "$offenders" = "__metadata_failed__" ]; then
      echo "::error file=Cargo.toml,title=依赖边界::cargo metadata 失败，无法校验 boa_engine 归属"
      fail=1
    elif [ -n "$offenders" ]; then
      echo "::error file=Cargo.toml,title=依赖边界::只有 webctl-js 可以依赖 boa_engine，违规：$offenders"
      fail=1
    fi
  fi
fi

# 3) 顺带核对：不得出现面向浏览器的依赖（名字里带 playwright/puppeteer 等）
if grep -qiE "^name = \".*(playwright|puppeteer|headless_chrome).*\"" "$lock"; then
  echo "::error file=Cargo.lock,title=依赖边界::出现了浏览器自动化依赖"
  fail=1
fi

if [ "$fail" -eq 0 ]; then echo "依赖边界检查通过"; fi
exit $fail
