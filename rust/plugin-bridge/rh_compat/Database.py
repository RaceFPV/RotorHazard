# Generated from src/server/Database.py by rust/tools/gen_rh_compat.py.
# Ships with the plugin bridge so plugins can import server internals on a
# machine that has no Python server. Values are sliced, not retyped.


class LapSource:
    REALTIME = 0
    MANUAL = 1
    RECALC = 2
    AUTOMATIC = 3
    API = 4


# ---------------------------------------------------------------------------
# Hand-written stand-ins for plugins that import model classes from Database.
# The originals are SQLAlchemy models; slicing them would drag in Base/DB, so a
# plugin gets value classes it can annotate with or construct from kwargs.
# gen_rh_compat.py appends this same block, so regenerating keeps it.
# ---------------------------------------------------------------------------
class _ModelStandIn:
    def __init__(self, **kwargs):
        for key, value in kwargs.items():
            setattr(self, key, value)

    def __repr__(self):
        fields = ', '.join('%s=%r' % item for item in vars(self).items())
        return '%s(%s)' % (type(self).__name__, fields)


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
