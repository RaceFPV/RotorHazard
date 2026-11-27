"""RHAPI: the class community plugins import for type hints.

The real RHAPI lives in the Python server and is constructed there. A plugin
never builds one - it is handed a live `rhapi` object by `initialize(rhapi)` -
so the bridge only has to make the import succeed. The attribute names below are
the ones the real object exposes, kept for plugins that annotate or introspect.
"""

RHAPI_VERSION = (1, 7)


class RHAPI:  # noqa: D101 - the docstring above is the documentation
    def __init__(self, *args, **kwargs):
        for name in (
            'ui', 'fields', 'db', 'race', 'config', 'utils', 'language',
            'led', 'vrxcontrol', 'classrank', 'points', 'heatgen', 'io',
            'interface', 'sensors', 'server', 'events',
        ):
            setattr(self, name, None)

    def __call__(self, message):  # plugins occasionally treat rhapi as __()
        return message
