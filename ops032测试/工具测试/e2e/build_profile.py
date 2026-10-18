#!/usr/bin/env python3
"""构建 dsh-ops/panel 端到端测试 profile：ops-panel-e2e。

- 基础：复制 ops-test（已装 dsh-ops 0.3.2 构建）
- bundles：base + headless + dsh-ops（含所有组件行）
- patch：复用 web profile 的 llm-pi-ai 提供商配置与默认模型，权限设为 danger-full-access/never，
        并把 dsh-ops-panel 的 capture 打开为 true。
"""
import json
import pathlib
import shutil

HOME = pathlib.Path.home()
SRC = HOME / ".dsh/profiles/ops-test"
DST = HOME / ".dsh/profiles/ops-panel-e2e"
WEB = HOME / ".dsh/profiles/web/cordis.patch.yml"
TGZ = "file:/home/thatgfsj/.dsh/artifacts/dsh-ops-0.3.2-linux-x64-f91b9ca.tgz"

DST.mkdir(parents=True, exist_ok=True)

for name in ("cordis.yml", "pnpm-workspace.yaml", "pnpm-lock.yaml"):
    shutil.copy2(SRC / name, DST / name)

if not (DST / "node_modules/dsh-ops").exists():
    shutil.copytree(SRC / "node_modules", DST / "node_modules", symlinks=True)

pkg = {
    "name": "dsh-profile-ops-panel-e2e",
    "private": True,
    "dsh": {
        "profile": {
            "bundles": [
                "@deepseek-ai/dsh-base",
                "@deepseek-ai/dsh-headless",
                "dsh-ops",
            ]
        }
    },
    "dependencies": {"dsh-ops": TGZ},
}
(DST / "package.json").write_text(
    json.dumps(pkg, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
)

lines = WEB.read_text(encoding="utf-8").splitlines(keepends=True)
start = next(i for i, l in enumerate(lines) if l.startswith("- id: llm-pi-ai"))
end = next(i for i, l in enumerate(lines) if l.startswith("- id: ui-conversation"))
llm_block = "".join(lines[start:end])

header = """# 端到端测试 profile（dsh-ops/panel capture 验收用，Lead 生成）
# 层序：base + headless + dsh-ops bundle 之后应用到本文件。
"""
perm_block = """- id: permission
  name: "@deepseek-ai/dsh-permission-presets"
  config:
    presets:
      read-only:
        sandbox: read-only
        approval: ask
      workspace-write:
        sandbox: workspace-write
        approval: ask
      danger-full-access:
        sandbox: danger-full-access
        approval: never
    defaultPreset: danger-full-access
"""
panel_block = """- id: dsh-ops-panel
  name: dsh-ops/panel
  config:
    capture: true
    textLimit: 2000
    maxSessionBytes: 8388608
"""
(DST / "cordis.patch.yml").write_text(
    header + llm_block + perm_block + panel_block, encoding="utf-8"
)
print("profile written:", DST)
print("patch bytes:", (DST / "cordis.patch.yml").stat().st_size)
