#!/usr/bin/env bash
# CI 失败定位与摘要。三平台通用：Linux/macOS 的 bash 与 Windows 上的 Git Bash 都能跑。
#
# 目标：把 cargo 的失败输出变成**精准的 GitHub 注解**（file:line:col），而不是让人在整段日志里翻。
# 解析三类信号：
#   1) 编译 / clippy：`error[EXXXX]: msg` + 紧随其后的 `--> path:line:col`；
#   2) 测试失败：`test NAME ... FAILED` + `thread ... panicked at path:line:col:` + 下一行的 panic 文案；
#   3) 构建失败汇总：`error: could not compile ...`。
#
# 用法：
#   scripts/ci-report.sh --lane <名字> --log <文件> [--workspace <仓库根>] [--max N]
#   scripts/ci-report.sh --selftest      # 用内置夹具自检（证明解析有判别力，不是摆设）
#
# 退出码：0 = 报告成功（不论日志里有没有失败）；--selftest 有任一断言不符则非 0。

set -uo pipefail

LANE=""; LOG=""; WS="${GITHUB_WORKSPACE:-$PWD}"; MAX=20; SELFTEST=0
while [ $# -gt 0 ]; do
  case "$1" in
    --lane) LANE="$2"; shift 2 ;;
    --log) LOG="$2"; shift 2 ;;
    --workspace) WS="$2"; shift 2 ;;
    --max) MAX="$2"; shift 2 ;;
    --selftest) SELFTEST=1; shift ;;
    *) echo "未知参数：$1" >&2; exit 2 ;;
  esac
done

# GitHub workflow command 转义（官方规则：% -> %25，换行/回车压成空格）
esc() { printf '%s' "$1" | tr '\n\r' '  ' | sed 's/%/%25/g'; }

# 路径规范化：任意平台路径 -> 仓库相对路径（Windows 的 D:\a\webctl\webctl\... 与 ./ 前缀）
REPO_NAME="$(basename "$WS")"
norm() {
  p="$1"
  p="${p//\\//}"
  p="${p#./}"
  case "$p" in
    # 主路径：锚定在 crates/ 上，**与检出目录叫什么无关**（夹具/CI 都不会因为目录名而误判）
    */crates/*) p="crates/${p##*/crates/}" ;;
    *)
      top="$(git rev-parse --show-toplevel 2>/dev/null || true)"
      top="${top//\\//}"
      if [ -n "$top" ] && [ "$p" != "${p#"$top"/}" ]; then
        p="${p#"$top"/}"
      else
        case "$p" in *"$REPO_NAME"/*) p="${p##*"$REPO_NAME"/}" ;; esac
      fi
      ;;
  esac
  printf '%s' "$p"
}

# ---------- 解析与报告 ----------
report() {
  log="$1"; lane="$2"
  [ -f "$log" ] || { echo "日志不存在：$log" >&2; return 2; }
  ann=0; npassed=0; nfailed=0; nignored=0; ncompile=0
  pend_msg=""; pend_kind=""; cur_test=""
  panic_want=0; panic_msg=""; pend_f=""; pend_ln=""; pend_co=""; pend_test=""
  rows="$(mktemp)"

  emit_panic() {
    printf '::error file=%s,line=%s,col=%s,title=%s::%s\n' \
      "$pend_f" "$pend_ln" "$pend_co" "$(esc "$lane · 测试用例失败")" "$(esc "$pend_test")"
    { printf '| %s:%s:%s | test | %s' "$pend_f" "$pend_ln" "$pend_co" "$(esc "$pend_test")"
      if [ -n "$panic_msg" ]; then printf ' —— %s' "$(esc "$panic_msg")"; fi
      printf ' |\n'; } >> "$rows"
    ann=$((ann+1)); panic_want=0
  }

  while IFS= read -r line || [ -n "$line" ]; do
    # 0) 上一行是 panic => 本行是 panic 文案，拿到就发
    if [ "$panic_want" -eq 1 ]; then
      case "$line" in
        "") ;;
        *) panic_msg="$line"; emit_panic ;;
      esac
    fi

    # 1) 测试汇总行：test result: FAILED. 3 passed; 1 failed; 0 ignored; ...
    case "$line" in
      *"test result:"*)
        p=$(printf '%s' "$line" | sed -n 's/.*\([0-9][0-9]*\) passed; \([0-9][0-9]*\) failed; \([0-9][0-9]*\) ignored.*/\1 \2 \3/p')
        if [ -n "$p" ]; then
          set -- $p
          npassed=$((npassed + $1)); nfailed=$((nfailed + $2)); nignored=$((nignored + $3))
        fi
        ;;
    esac

    # 2) 编译 / clippy 错误头（错误码可选）
    if printf '%s' "$line" | grep -qE '^error(\[[A-Za-z0-9]+\])?: '; then
      pend_kind=$(printf '%s' "$line" | sed -n 's/^error\(\[[A-Za-z0-9]*\]\)\?:.*/\1/p')
      pend_msg=$(printf '%s' "$line" | sed 's/^error\(\[[A-Za-z0-9]*\]\)\?: *//')
    fi

    # 3) 位置行 --> path:line:col（紧随错误头）
    case "$line" in
      *"-->"*)
        loc=$(printf '%s' "$line" | sed -n 's/.*--> *\(.*\):\([0-9][0-9]*\):\([0-9][0-9]*\).*/\1|\2|\3/p')
        if [ -n "$loc" ] && [ -n "$pend_msg" ]; then
          f=$(norm "$(printf '%s' "$loc" | cut -d'|' -f1)")
          ln=$(printf '%s' "$loc" | cut -d'|' -f2); co=$(printf '%s' "$loc" | cut -d'|' -f3)
          printf '::error file=%s,line=%s,col=%s,title=%s::%s\n' "$f" "$ln" "$co" "$(esc "$lane · ${pend_kind:-compile}")" "$(esc "$pend_msg")"
          printf '| %s:%s:%s | %s | %s |\n' "$f" "$ln" "$co" "${pend_kind:-compile}" "$(esc "$pend_msg")" >> "$rows"
          ann=$((ann+1)); pend_msg=""
        fi
        ;;
    esac

    # 4) 失败用例名
    case "$line" in
      "test "*" ... FAILED"*)
        cur_test=$(printf '%s' "$line" | sed -n 's/^test \(.*\) \.\.\. FAILED.*/\1/p')
        ;;
    esac

    # 5) panic 位置（标题带用例名；文案在下一行，见步骤 0）
    case "$line" in
      "thread "*" panicked at "*)
        loc=$(printf '%s' "$line" | sed -n 's/^thread .* panicked at \(.*\):\([0-9][0-9]*\):\([0-9][0-9]*\):.*/\1|\2|\3/p')
        if [ -n "$loc" ]; then
          pend_f=$(norm "$(printf '%s' "$loc" | cut -d'|' -f1)")
          pend_ln=$(printf '%s' "$loc" | cut -d'|' -f2)
          pend_co=$(printf '%s' "$loc" | cut -d'|' -f3)
          pend_test="${cur_test:-?}"; panic_msg=""; panic_want=1
        fi
        ;;
    esac

    # 6) 构建失败汇总
    case "$line" in
      "error: could not compile"*|"error: internal compiler error"*) ncompile=$((ncompile+1)) ;;
    esac
  done < "$log"

  if [ "$panic_want" -eq 1 ]; then emit_panic; fi   # 日志到末尾也没等到文案

  echo "== [$lane] 失败定位 =="
  if [ "$ann" -eq 0 ]; then echo "  未发现可定位的失败（编译错误 / 测试 panic）"; fi
  head -n "$MAX" "$rows"
  echo "  统计：passed=$npassed failed=$nfailed ignored=$nignored 注解=$ann 构建汇总失败=$ncompile"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
      echo "### [$lane] 失败定位"
      echo
      echo "| 指标 | 值 |"
      echo "|---|---|"
      echo "| passed | $npassed |"
      echo "| failed | $nfailed |"
      echo "| ignored | $nignored |"
      echo "| 可定位注解 | $ann |"
      if [ "$ann" -gt 0 ]; then echo; echo "| 位置 | 类型 | 说明 |"; echo "|---|---|---|"; head -n "$MAX" "$rows"; fi
    } >> "$GITHUB_STEP_SUMMARY"
  fi
  rm -f "$rows"
  return 0
}

# ---------- 自检（植入式夹具：把关键行改掉，定位必须跟着变） ----------
selftest() {
  tmp="$(mktemp -d)"; fails=0

  cat > "$tmp/clippy.log" <<'EOF'
   Compiling webctl-web v0.1.1
error[E0308]: mismatched types
   --> crates/webctl-web/src/parse.rs:42:9
    |
 42 |     let x: u32 = "s";
EOF

  cat > "$tmp/test.log" <<'EOF'
running 4 tests
test mcp_09_call_budget_times_out_as_envelope ... ok
test sec_09_mcp_dispatcher_is_the_one_that_redacts ... FAILED
thread 'dispatch::tests::sec_09' panicked at D:\a\webctl\webctl\crates\webctl-mcp\src\dispatch.rs:791:9:
MCP 侧脱敏未生效，仍可见明文

test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
error: test failed, to rerun pass '--lib'
EOF

  cat > "$tmp/clean.log" <<'EOF'
   Compiling webctl v0.1.1
    Finished `test` profile [unoptimized] target(s) in 12.34s
running 2 tests
test a ... ok
test b ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
EOF

  expect() { # 描述 实际 期望子串
    case "$2" in
      *"$3"*) echo "  PASS $1" ;;
      *) echo "  FAIL $1（未出现：$3）"; fails=$((fails+1)) ;;
    esac
  }
  reject() { # 描述 实际 不该出现的子串
    case "$2" in
      *"$3"*) echo "  FAIL $1（不该出现：$3）"; fails=$((fails+1)) ;;
      *) echo "  PASS $1" ;;
    esac
  }

  # 关键：显式用一个**不叫 webctl** 的工作区，证明定位与检出目录名无关
  WS=/tmp/definitely-not-webctl
  out="$(report "$tmp/clippy.log" clippy)"
  expect "clippy 定位到 file:line:col" "$out" "file=crates/webctl-web/src/parse.rs,line=42,col=9"
  expect "clippy 错误码带出" "$out" "E0308"

  out="$(report "$tmp/test.log" test)"
  expect "测试定位（Windows 路径已规范化）" "$out" "file=crates/webctl-mcp/src/dispatch.rs,line=791,col=9"
  expect "失败用例名带出" "$out" "sec_09_mcp_dispatcher_is_the_one_that_redacts"
  expect "计数解析" "$out" "failed=1"
  expect "panic 文案带出" "$out" "MCP 侧脱敏未生效"

  out="$(report "$tmp/clean.log" test)"
  reject "干净日志零误报" "$out" "::error"

  WS=/tmp/another-name-entirely
  out="$(report "$tmp/test.log" test)"
  expect "定位与检出目录名解耦（换工作区名仍正确）" "$out" "file=crates/webctl-mcp/src/dispatch.rs,line=791,col=9"

  sed 's/791:9/999:1/' "$tmp/test.log" > "$tmp/test2.log"
  out="$(report "$tmp/test2.log" test)"
  expect "行号随夹具变化（非硬编码）" "$out" "line=999"

  rm -rf "$tmp"
  if [ "$fails" -eq 0 ]; then echo "自检：全部通过"; return 0; fi
  echo "自检：$fails 项未通过" >&2; return 1
}

if [ "$SELFTEST" -eq 1 ]; then selftest; exit $?; fi
report "$LOG" "${LANE:-ci}"
exit $?
