#!/usr/bin/env python3
"""工具面一致性守卫（议题 #114 引入）。

为什么需要它：工具数从 15 变 16 时，"某个文档/注释里还写着旧数字"这类漂移靠人眼反复漏
（本仓已漏三轮）。这里机械校验：

  1) TOOL_NAMES 的名字与顺序（唯一真源）；
  2) PLAN §3.7 表格：行数 = N 且名字/顺序与 TOOL_NAMES 逐一同；
  3) SKILL.md：标题"（N 个"与 tools:start..end 清单 = TOOL_NAMES；
  4) 现行文档与 crates 注释里的"N 个工具 / N 工具 / N tools / N 个裸名 / 恰好 N 个 / 把 N 个名字"
     必须等于 N —— 除非该行带历史标记（PR 11 / 当时 / #114 起 / v0.1.）。

用法：scripts/check-tool-face.py [--root <目录>]    （--root 供自测/变异使用）
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

HIST = re.compile(r"PR 11|当时|#114 起|v0\.1\.")
PROSE = re.compile(
    r"(?<![0-9.])([0-9]+)\s*(?:个工具|工具|tools|-tool|个裸名)"
    r"|(?<![0-9.])恰好\s*[0-9]+\s*个"
    r"|(?<![0-9.])把\s*[0-9]+\s*个名字"
)
NUMBER = re.compile(r"[0-9]+")

SCAN_FILES = [
    "README.md",
    "SKILL.md",
    "docs/DSH-INTEGRATION.md",
    "docs/CONFIGURATION.md",
    "docs/CI.md",
]


def fail(problems: list[str], message: str) -> None:
    problems.append(message)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", default=None)
    args = parser.parse_args()
    root = Path(args.root).resolve() if args.root else Path(__file__).resolve().parent.parent

    tools_rs = root / "crates/webctl-mcp/src/tools.rs"
    plan = root / "docs/PLAN.md"
    skill = root / "SKILL.md"
    for path in (tools_rs, plan, skill):
        if not path.is_file():
            print(f"缺少文件：{path}", file=sys.stderr)
            return 2

    # 1) 真源：TOOL_NAMES
    source = tools_rs.read_text(encoding="utf-8")
    block = re.search(r"pub const TOOL_NAMES[^=]*=\s*\[(.*?)\];", source, re.S)
    if not block:
        print("无法从 TOOL_NAMES 解析出名字", file=sys.stderr)
        return 2
    names = re.findall(r"\"([a-z_]+)\"", block.group(1))
    count = len(names)
    if count == 0:
        print("TOOL_NAMES 为空", file=sys.stderr)
        return 2

    problems: list[str] = []

    # 2) PLAN §3.7
    plan_text = plan.read_text(encoding="utf-8")
    section = re.search(r"### 3\.7(.*?)### 3\.8", plan_text, re.S)
    plan_names: list[str] = []
    if section:
        plan_names = re.findall(r"^\|\s*[0-9]+\s*\|\s*`([a-z_]+)`", section.group(1), re.M)
    if len(plan_names) != count:
        fail(problems, f"::error file=docs/PLAN.md,title=工具面::§3.7 表格 {len(plan_names)} 行，但 TOOL_NAMES 有 {count} 个")
    elif plan_names != names:
        fail(problems, "::error file=docs/PLAN.md,title=工具面::§3.7 名字或顺序与 TOOL_NAMES 不一致")

    # 3) SKILL.md
    skill_text = skill.read_text(encoding="utf-8")
    declared = re.search(r"^## 工具（([0-9]+) 个", skill_text, re.M)
    if not declared or int(declared.group(1)) != count:
        got = declared.group(1) if declared else "?"
        fail(problems, f"::error file=SKILL.md,title=工具面::标题写 {got} 个工具，实际 {count} 个")
    listed = re.search(r"<!-- tools:start -->(.*?)<!-- tools:end -->", skill_text, re.S)
    skill_names = re.findall(r"^- `([a-z_]+)`", listed.group(1), re.M) if listed else []
    if len(skill_names) != count:
        fail(problems, f"::error file=SKILL.md,title=工具面::清单 {len(skill_names)} 个，实际 {count} 个")
    elif skill_names != names:
        fail(problems, "::error file=SKILL.md,title=工具面::清单名字或顺序与 TOOL_NAMES 不一致")

    # 3b) 每个工具名都必须在 README 与 SKILL.md 里出现（防止清单漏项）
    for rel in ("README.md", "SKILL.md"):
        path = root / rel
        if not path.is_file():
            continue
        body = path.read_text(encoding="utf-8")
        missing = [name for name in names if f"`{name}`" not in body]
        if missing:
            fail(problems, f"::error file={rel},title=工具面::清单缺少 {len(missing)} 个工具：{", ".join(missing)}")

    # 3c) README 工具清单：把"工具面定稿为 N 个内置工具"那一段的名单解析出来比对
    readme = root / "README.md"
    if readme.is_file():
        lines = readme.read_text(encoding="utf-8").splitlines()
        for index, line in enumerate(lines):
            if "工具面定稿为" not in line:
                continue
            # 只吃"工具面定稿为"那一段的清单：遇到 CLI 子命令段或空行就停
            listed: set[str] = set()
            # 从下一行开始：intro 行本身可能括注提到某个工具名（例如"（`embedded_data` 于 0.1.2 加入）"）
            for follow in lines[index + 1 :]:
                if "CLI 子命令" in follow or not follow.strip():
                    break
                listed.update(re.findall(r"`([a-z_]+)`", follow))
            wanted = set(names)
            if listed != wanted:
                missing = sorted(wanted - listed)
                extra = sorted(listed - wanted)
                fail(
                    problems,
                    f"::error file=README.md,line={index + 1},title=工具面::工具清单与 TOOL_NAMES 不一致"
                    f"（缺 {missing}，多 {extra}）",
                )
            break

    # 4) 现行文档文案
    for rel in SCAN_FILES:
        path = root / rel
        if not path.is_file():
            continue
        problems.extend(scan_lines(path.read_text(encoding="utf-8"), rel, count))

    # 5) crates 注释与清单
    for path in sorted(list((root / "crates").rglob("*.rs")) + list((root / "crates").rglob("*.toml"))):
        rel = path.relative_to(root).as_posix()
        problems.extend(scan_lines(path.read_text(encoding="utf-8"), rel, count))

    if problems:
        for problem in problems:
            print(problem)
        print(f"工具面不一致：{len(problems)} 处（真源 = TOOL_NAMES，共 {count} 个）", file=sys.stderr)
        return 1
    print(f"工具面一致：TOOL_NAMES = PLAN §3.7 = SKILL.md = {count} 个")
    return 0


def scan_lines(text: str, rel: str, count: int) -> list[str]:
    """找出"现行文案里写错工具数"的行（历史标记行豁免）。"""
    out: list[str] = []
    for index, raw_line in enumerate(text.splitlines(), start=1):
        line = raw_line.replace("*", "").replace("`", "")
        if HIST.search(raw_line):
            continue
        if not PROSE.search(line):
            continue
        numbers = set()
        for match in PROSE.finditer(line):
            numbers.update(int(n) for n in NUMBER.findall(match.group(0)))
        for got in sorted(numbers):
            if got != count:
                out.append(
                    f"::error file={rel},line={index},title=工具面::这里写 {got} 个工具，实际 {count} 个"
                    "（历史记录请带 PR 11 / 当时 / #114 起 / v0.1. 标记）"
                )
    return out


if __name__ == "__main__":
    sys.exit(main())
