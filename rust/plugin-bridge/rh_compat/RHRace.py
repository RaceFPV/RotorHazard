# Generated from src/server/RHRace.py by rust/tools/gen_rh_compat.py.
# Ships with the plugin bridge so plugins can import server internals on a
# machine that has no Python server. Values are sliced, not retyped.


class RaceStatus():
    READY = 0
    STAGING = 3
    RACING = 1
    DONE = 2
