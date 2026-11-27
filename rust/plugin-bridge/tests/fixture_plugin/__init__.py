'''Fixture plugin for the bridge test suite.

Not a real plugin: it exercises the RHAPI surface and reports what works, so a
change that breaks the bridge fails a fast, self-contained test instead of
waiting for a user's plugin to break.
'''

import json
import os


def _probe(rhapi):
    """Call a representative slice of RHAPI; return (ok, failures)."""
    ok = []
    failures = []

    def check(name, fn):
        try:
            fn()
            ok.append(name)
        except Exception as exc:  # noqa: BLE001 - the report is the point
            failures.append({'call': name, 'error': '%s: %s' % (type(exc).__name__, exc)})

    check('utils.format_time_to_str', lambda: rhapi.utils.format_time_to_str(125250))
    check('utils.format_split_time_to_str',
          lambda: rhapi.utils.format_split_time_to_str(125250))
    check('utils.color_hsl_to_hexstring', lambda: rhapi.utils.color_hsl_to_hexstring(0, 100, 50))
    check('utils.color_hexstring_to_int', lambda: rhapi.utils.color_hexstring_to_int('#ff0000'))
    check('utils.band_channel_frequencies', lambda: rhapi.utils.band_channel_frequencies)
    check('utils.generate_unique_name',
          lambda: rhapi.utils.generate_unique_name('Heat', ['Heat']))

    check('server.data_dir', lambda: rhapi.server.data_dir)
    check('server.info', lambda: rhapi.server.info)
    check('server.info.version', lambda: rhapi.server.info['version'])
    check('server.plugins', lambda: rhapi.server.plugins)
    check('server.seat_color_defaults', lambda: rhapi.server.seat_color_defaults)
    check('language.languages', lambda: rhapi.language.languages)
    check('translate', lambda: rhapi.__('Heat'))

    check('config.get', lambda: rhapi.config.get('UI', 'timerName'))
    check('config.get_all', lambda: rhapi.config.get_all)

    check('db.pilots', lambda: rhapi.db.pilots)
    check('db.heats', lambda: rhapi.db.heats)
    check('db.races', lambda: rhapi.db.races)
    check('db.raceclasses', lambda: rhapi.db.raceclasses)
    check('db.raceformats', lambda: rhapi.db.raceformats)
    check('db.frequencysets', lambda: rhapi.db.frequencysets)
    check('db.options', lambda: rhapi.db.options)
    check('db.slots', lambda: rhapi.db.slots)
    check('db.pilotruns', lambda: rhapi.db.pilotruns)
    check('db.laps', lambda: rhapi.db.laps)
    check('db.backup_list', lambda: rhapi.db.backup_list)
    check('db.option', lambda: rhapi.db.option('_fixture_flag'))

    # Objects must look like Python's: every row carries `.id`, plus its own FK
    # fields under the same names the Python models use.
    def rows_have_ids():
        for name in ('pilots', 'heats', 'races', 'raceclasses', 'raceformats',
                     'frequencysets', 'slots', 'pilotruns'):
            rows = getattr(rhapi.db, name)
            if not rows:
                continue
            row = rows[0]
            if getattr(row, 'id', None) is None:
                raise AssertionError('%s row has no .id (keys: %r)'
                                     % (name, list(row.keys())))

    check('db rows expose .id', rows_have_ids)

    def object_attributes():
        for heat in rhapi.db.heats:
            heat.display_name if hasattr(heat, 'display_name') else None
        for fmt in rhapi.db.raceformats:
            if getattr(fmt, 'unlimited_time', None) is None:
                raise AssertionError('raceformat has no unlimited_time')

    check('db objects expose python attributes', object_attributes)

    check('race.status', lambda: rhapi.race.status)
    check('race.pilots', lambda: rhapi.race.pilots)
    check('race.slots', lambda: rhapi.race.slots)
    check('race.seat_colors', lambda: rhapi.race.seat_colors)
    check('race.laps', lambda: rhapi.race.laps)
    check('race.race_time_sec', lambda: rhapi.race.race_time_sec)
    check('race.scheduled', lambda: rhapi.race.scheduled)

    check('led.enabled', lambda: rhapi.led.enabled)
    check('led.effects', lambda: rhapi.led.effects)
    check('vrxcontrol.enabled', lambda: rhapi.vrxcontrol.enabled)
    check('classrank.methods', lambda: rhapi.classrank.methods)
    check('points.methods', lambda: rhapi.points.methods)
    check('heatgen.generators', lambda: rhapi.heatgen.generators)
    check('io.exporters', lambda: rhapi.io.exporters)
    check('interface.seats', lambda: rhapi.interface.seats)
    check('sensors.sensor_names', lambda: rhapi.sensors.sensor_names)

    return ok, failures


class FixturePlugin:
    def __init__(self, rhapi):
        self._rhapi = rhapi
        self.laps_seen = 0
        self.clicks = 0
        self._rhapi.events.on('raceLapRecorded', self.on_lap)

    def initialize(self, _args):
        rhapi = self._rhapi
        rhapi.ui.register_panel('fixture', 'Fixture Plugin', 'settings', order=1)

        from RHUI import UIField, UIFieldType
        rhapi.fields.register_option(
            UIField('_fixture_flag', 'Fixture Flag', field_type=UIFieldType.CHECKBOX),
            'fixture')
        rhapi.fields.register_race_attribute(UIField('fixture_race', 'Fixture Race', UIFieldType.TEXT))
        rhapi.fields.register_pilot_attribute(UIField('fixture_pilot', 'Fixture Pilot', UIFieldType.TEXT))

        # A blueprint with two routes: one JSON, one plain text with a status,
        # so the server-side proxy and the response mapping are both exercised.
        from flask import Blueprint, jsonify, request
        bp = Blueprint('fixture_routes', __name__)

        @bp.route('/fixture/ping/<int:number>', methods=['GET'])
        def ping(number):
            return jsonify({'pong': number, 'q': request.args.get('q')})

        @bp.route('/fixture/text', methods=['GET'])
        def text():
            return 'plain text', 201

        rhapi.ui.blueprint_add(bp)

        # A blueprint with two routes: one JSON, one plain text with a status,
        # so the server-side proxy and the response mapping are both exercised.
        from flask import Blueprint, jsonify, request
        bp = Blueprint('fixture_routes', __name__)

        @bp.route('/fixture/ping/<int:number>', methods=['GET'])
        def ping(number):
            return jsonify({'pong': number, 'q': request.args.get('q')})

        @bp.route('/fixture/text', methods=['GET'])
        def text():
            return 'plain text', 201

        rhapi.ui.blueprint_add(bp)

        # A blueprint with two routes: one JSON, one plain text with a status,
        # so the server-side proxy and the response mapping are both exercised.
        from flask import Blueprint, jsonify, request
        bp = Blueprint('fixture_routes', __name__)

        @bp.route('/fixture/ping/<int:number>', methods=['GET'])
        def ping(number):
            return jsonify({'pong': number, 'q': request.args.get('q')})

        @bp.route('/fixture/text', methods=['GET'])
        def text():
            return 'plain text', 201

        rhapi.ui.blueprint_add(bp)

        # A blueprint with two routes: one JSON, one plain text with a status,
        # so the server-side proxy and the response mapping are both exercised.
        from flask import Blueprint, jsonify, request
        bp = Blueprint('fixture_routes', __name__)

        @bp.route('/fixture/ping/<int:number>', methods=['GET'])
        def ping(number):
            return jsonify({'pong': number, 'q': request.args.get('q')})

        @bp.route('/fixture/text', methods=['GET'])
        def text():
            return 'plain text', 201

        rhapi.ui.blueprint_add(bp)

        # A blueprint with two routes: one JSON, one plain text with a status,
        # so the server-side proxy and the response mapping are both exercised.
        from flask import Blueprint, jsonify, request
        bp = Blueprint('fixture_routes', __name__)

        @bp.route('/fixture/ping/<int:number>', methods=['GET'])
        def ping(number):
            return jsonify({'pong': number, 'q': request.args.get('q')})

        @bp.route('/fixture/text', methods=['GET'])
        def text():
            return 'plain text', 201

        rhapi.ui.blueprint_add(bp)

        # A blueprint with two routes: one JSON, one plain text with a status,
        # so the server-side proxy and the response mapping are both exercised.
        from flask import Blueprint, jsonify, request
        bp = Blueprint('fixture_routes', __name__)

        @bp.route('/fixture/ping/<int:number>', methods=['GET'])
        def ping(number):
            return jsonify({'pong': number, 'q': request.args.get('q')})

        @bp.route('/fixture/text', methods=['GET'])
        def text():
            return 'plain text', 201

        rhapi.ui.blueprint_add(bp)

        rhapi.ui.register_markdown('fixture', 'fixture_note',
                                   'Fixture **markdown**: panel content reaches the UI.')
        rhapi.ui.register_quickbutton('fixture', 'fixture_action', 'Fixture Action',
                                      self.quick_action)

        rhapi.ui.socket_listen('ts_probe', self.probe)
        rhapi.ui.socket_listen('ts_fixture_stage', self.stage)
        rhapi.ui.socket_listen('ts_fixture_write', self.write_option)

    def quick_action(self, _arg=None):
        '''Runs when the panel button is clicked (dispatch_event).'''
        self.clicks += 1
        return {'clicked': True, 'clicks': self.clicks}

    def probe(self, _arg=None):
        ok, failures = _probe(self._rhapi)
        return {'ok_count': len(ok), 'ok': ok, 'failures': failures,
                'laps_seen': self.laps_seen,
                'version': self._rhapi.server.info.get('version')}

    def write_option(self, arg=None):
        '''Round-trip a checkbox the way the settings page does.'''
        value = bool((arg or {}).get('value', True))
        self._rhapi.db.option_set('_fixture_flag', value)
        return {'stored': self._rhapi.db.option('_fixture_flag')}

    def stage(self, arg=None):
        '''Stage a race through RHAPI, as a real connector would.'''
        rhapi = self._rhapi
        if rhapi.race.status != 0:
            rhapi.race.stop()
            rhapi.race.clear()
        heat = rhapi.db.heat_add(name='Fixture Heat')
        slots = rhapi.db.slots_by_heat(heat.id)
        pilots = []
        for index, callsign in enumerate(['Probe One', 'Probe Two']):
            pilot = rhapi.db.pilot_add(name=callsign, callsign=callsign)
            pilots.append(pilot.id)
            for slot in slots:
                if slot.node_index == index:
                    rhapi.db.slot_alter(slot.id, pilot=pilot.id)
                    break
        rhapi.race.heat = heat.id
        rhapi.race.stage({})
        return {'heat': heat.id, 'pilots': pilots}

    def on_lap(self, args):
        self.laps_seen += 1
        self._rhapi.ui.socket_broadcast('ts_fixture_lap', {
            'seat': args.get('node_index'),
            'lap_time_stamp': (args.get('lap') or {}).get('lap_time_stamp')
            if isinstance(args.get('lap'), dict) else None,
        })


def initialize(rhapi):
    plugin = FixturePlugin(rhapi)
    rhapi.events.on('startup', plugin.initialize)
