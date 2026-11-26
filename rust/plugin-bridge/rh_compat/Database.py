# Generated from src/server/Database.py by rust/tools/gen_rh_compat.py.
# Ships with the plugin bridge so plugins can import server internals on a
# machine that has no Python server. Values are sliced, not retyped.


class LapSource:
    REALTIME = 0
    MANUAL = 1
    RECALC = 2
    AUTOMATIC = 3
    API = 4
