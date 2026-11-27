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


DATABASE_STANDINS = """


# ---------------------------------------------------------------------------
# Hand-written stand-ins for plugins that import model classes from Database.
# The originals are SQLAlchemy models; slicing them would drag in Base/DB, so a
# plugin gets value classes it can annotate with or construct from kwargs.
# ---------------------------------------------------------------------------
class _ModelStandIn:
    def __init__(self, **kwargs):
        for key, value in kwargs.items():
            setattr(self, key, value)


class Pilot(_ModelStandIn):
    pass


class SavedRaceLap(_ModelStandIn):
    pass


class Heat(_ModelStandIn):
    pass


class RaceClass(_ModelStandIn):
    pass


class RaceFormat(_ModelStandIn):
    pass


class Race(_ModelStandIn):
    pass


class Profile(_ModelStandIn):
    pass


class GlobalSettings(_ModelStandIn):
    pass


class Option(_ModelStandIn):
    pass


class PilotRun(_ModelStandIn):
    pass


class Lap(_ModelStandIn):
    pass
"""

os.makedirs(OUT, exist_ok=True)
HEADER = """# Generated from src/server/{src} by rust/tools/gen_rh_compat.py.
# Ships with the plugin bridge so plugins can import server internals on a
# machine that has no Python server. Values are sliced, not retyped.
"""

# Modules copied whole: RHUtils and frequencies are pure Python (no server
# imports beyond util.RH_GPIO, which is stubbed) and RHAPI.utils delegates to
# them, so shipping the real code is the only way to keep parity.
WHOLE_MODULES = ["RHUtils.py", "frequencies.py"]

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
        if dest == 'Database.py':
            body.append(DATABASE_STANDINS)
    target = os.path.join(OUT, out_name)
    with open(target, "w", encoding="utf-8", newline="\n") as handle:
        handle.write("\n\n".join(body) + "\n")
    print("wrote", target)

for name in WHOLE_MODULES:
    src = open(os.path.join(SRC, name), encoding="utf-8").read()
    target = os.path.join(OUT, name)
    with open(target, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(HEADER.format(src=name) + "\n" + src)
    print("wrote", target)

# RHUtils imports util.RH_GPIO for the Pi GPIO helpers; those paths are not
# used by the plugin-facing functions, so a stub keeps the import working.
UTIL = os.path.join(OUT, "util")
os.makedirs(UTIL, exist_ok=True)
open(os.path.join(UTIL, "__init__.py"), "w").write("")
open(os.path.join(UTIL, "RH_GPIO.py"), "w", encoding="utf-8", newline="\n").write(
    '"""Stub: the Rust server has no GPIO backend; RHUtils only calls these\n'
    'from hardware-specific helpers."""\n\n\n'
    "def check_input_tied_low(pin):\n    return False\n\n\n"
    "def is_real_hw_GPIO():\n    return False\n\n\n"
    "def get_GPIO_type_str():\n    return 'none'\n")
print("wrote util/RH_GPIO.py stub")

open(os.path.join(OUT, "__init__.py"), "w").write(
    "# Server-internal shims for plugins loaded by rhapi_bridge.py.\n")
print("wrote __init__.py")
