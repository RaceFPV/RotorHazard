"""Generate rust/plugin-bridge/rh_compat from the Python server's own source.

Plugins import server internals (RHRace.RaceStatus, eventmanager.Evt,
RHUI.UIField*, RHUtils.HEAT_ID_NONE, Database.LapSource). A Rust deployment has
no Python tree, so the bridge ships sliced copies of exactly those symbols,
extracted by AST so the values cannot drift from the originals by transcription.
"""
import ast
import os

SRC = "/home/racefpv/git/RotorHazard/src/server"
OUT = "/home/racefpv/git/RotorHazard/rust/plugin-bridge/rh_compat"

TYPING_PRELUDE = (
    "from enum import Enum\n"
    "from typing import Any, Optional, Dict, List, Callable\n"
    "from dataclasses import dataclass, field\n"
)


def slice_symbol(path, name):
    """Return the source lines defining a top-level class or simple assignment."""
    text = open(path, encoding="utf-8").read()
    lines = text.splitlines()
    tree = ast.parse(text)
    for node in tree.body:
        if isinstance(node, ast.ClassDef) and node.name == name:
            start = node.lineno - 1
            if node.decorator_list:
                # @dataclass sits above the class statement and carries the
                # generated __init__; dropping it made UIField() take no args.
                start = min(d.lineno - 1 for d in node.decorator_list)
            return "\n".join(lines[start:node.end_lineno])
        if isinstance(node, ast.Assign):
            for target in node.targets:
                if isinstance(target, ast.Name) and target.id == name:
                    return "\n".join(lines[node.lineno - 1:node.end_lineno])
    raise SystemExit("symbol %s not found in %s" % (name, path))


os.makedirs(OUT, exist_ok=True)
HEADER = """# Generated from src/server/{src} by rust/tools/gen_rh_compat.py.
# Ships with the plugin bridge so plugins can import server internals on a
# machine that has no Python server. Values are sliced, not retyped.
"""

MODULES = {
    "RHRace.py": ("RHRace.py", ["RaceStatus"], ""),
    "RHUtils.py": ("RHUtils.py", ["HEAT_ID_NONE"], ""),
    "Database.py": ("Database.py", ["LapSource"], ""),
    "eventmanager.py": ("eventmanager.py", ["Evt"], ""),
    "RHUI.py": ("RHUI.py", ["UIFieldType", "UIFieldSelectOption", "UIField"],
                TYPING_PRELUDE),
}

for out_name, (src_name, symbols, prelude) in MODULES.items():
    body = [HEADER.format(src=src_name)]
    if prelude:
        body.append(prelude.rstrip("\n"))
    for symbol in symbols:
        body.append(slice_symbol(os.path.join(SRC, src_name), symbol))
    target = os.path.join(OUT, out_name)
    with open(target, "w", encoding="utf-8", newline="\n") as handle:
        handle.write("\n\n".join(body) + "\n")
    print("wrote", target)

open(os.path.join(OUT, "__init__.py"), "w").write(
    "# Server-internal shims for plugins loaded by rhapi_bridge.py.\n")
print("wrote __init__.py")
