#!/usr/bin/env python3
'''
RotorHazard plugin bridge — runs third-party RHAPI plugins against the
Rust server, out-of-process.

The Rust server is pure Rust; this sidecar loads user plugins (unmodified
RHAPI code), forwards their RHAPI calls to the server over a localhost
socket.io RPC endpoint ("rhapi_call"), and delivers server events to the
hooks the plugins register.

Usage:
    python3 rhapi_bridge.py [--server http://127.0.0.1:5000] \
        [--plugins-dir ./plugins] [--plugins name1,name2] [--control-port 5099]

The server is never required to run Python; if this bridge isn't
running, the server behaves as if no plugins are installed.
'''
import argparse
import importlib.util
import json
import os
import re
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import socketio

# Plugins import RotorHazard server internals (RHRace.RaceStatus,
# eventmanager.Evt, RHUI.UIField, RHUtils.HEAT_ID_NONE, Database.LapSource).
# A machine running the Rust server has no Python tree, so rh_compat ships
# sliced copies of exactly those symbols (see rust/tools/gen_rh_compat.py).
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                'rh_compat'))

# Plugins import flask at module level to register a blueprint. The Python server
# serves those routes; this one does not yet, so a plugin that needs flask gets a
# stand-in that records the blueprint and routes nothing - it still loads, and
# its panels and buttons work. A real flask, if installed, wins.
try:
    import flask  # noqa: F401
except ImportError:
    import flask_shim

    sys.modules['flask'] = flask_shim
    sys.modules['flask.blueprints'] = flask_shim

RPC_OBJECT = 'rhapi_call'
# Internal event used by the server to run a plugin's HTTP route; kept in sync
# with the Rust side (crash-free if either is missing: it just 404s).
ROUTE_EVENT = '__rh_route'
RPC_TIMEOUT = 10.0


# --------------------------------------------------------------------------
# Values returned by the server are plain JSON; RHAPI hands plugins objects
# with attributes (pilot.id, heat.display_name), so wrap them on the way in.
# --------------------------------------------------------------------------
def wrap(value):
    if isinstance(value, dict):
        return Obj(value)
    if isinstance(value, list):
        return [wrap(item) for item in value]
    return value



def _match_rule(rule, path):
    """Flask-style rule matching: <int:x>, <float:x>, <string:x>, <path:x>, <x>."""
    rule_parts = [part for part in rule.strip('/').split('/') if part]
    path_parts = [part for part in path.strip('/').split('/') if part]
    params = {}
    for position, part in enumerate(rule_parts):
        if part.startswith('<') and part.endswith('>'):
            spec = part[1:-1]
            converter, _, name = spec.partition(':')
            if not name:
                converter, name = 'string', spec
            if converter == 'path':
                rest = '/'.join(path_parts[position:])
                if not rest:
                    return None
                params[name] = rest
                return params
            if position >= len(path_parts):
                return None
            value = path_parts[position]
            # Convert like Flask does: a view declared <int:x> must receive an
            # int, not the matched text. The real-Flask path gets this from
            # Flask's router; without Flask (the shim) it has to happen here or
            # jsonify returns "7" where Python returns 7.
            if converter == 'int':
                try:
                    value = int(value)
                except ValueError:
                    return None
            elif converter == 'float':
                try:
                    value = float(value)
                except ValueError:
                    return None
            params[name] = value
        elif position >= len(path_parts) or path_parts[position] != part:
            return None
    if len(path_parts) != len(rule_parts):
        return None
    return params


def _set_request(args, params):
    """Points flask.request at this call; returns what to restore afterwards."""
    import flask_shim
    request = flask_shim.request
    previous = {key: getattr(request, key, None)
                for key in ('method', 'path', 'args', 'json', 'data', 'headers', 'form')}
    request.method = str(args.get('method', 'GET')).upper()
    request.path = str(args.get('path', '/'))
    request.args = dict(args.get('query') or {})
    request.data = str(args.get('body') or '').encode('utf-8')
    request.headers = dict(args.get('headers') or {})
    request.form = {}
    request.json = None
    if 'application/json' in str(request.headers.get('content-type', '')).lower():
        try:
            request.json = json.loads(request.data or b'{}')
        except ValueError:
            request.json = None
    return previous


def _restore_request(previous):
    import flask_shim
    for key, value in previous.items():
        setattr(flask_shim.request, key, value)


def _normalise_response(result):
    """What a Flask view returns -> {status, content_type, body}."""
    status = 200
    headers = {}

    if isinstance(result, tuple):
        body = result[0]
        if len(result) >= 2:
            if isinstance(result[1], (int, float)):
                status = int(result[1])
            elif isinstance(result[1], dict):
                headers = dict(result[1])
        if len(result) >= 3 and isinstance(result[2], dict):
            headers = dict(result[2])
        result = body

    # a real Flask/Werkzeug response carries its own status, headers and bytes
    if hasattr(result, 'get_data'):
        status = int(getattr(result, 'status_code', status) or status)
        for key, value in (getattr(result, 'headers', None) or {}).items():
            headers[str(key)] = str(value)
        content_type = headers.get('Content-Type',
                                   headers.get('content-type',
                                               'text/html; charset=utf-8'))
        if getattr(result, 'direct_passthrough', False):
            # send_file/send_from_directory: get_data() refuses, the body is the
            # iterable itself (static JS, images, fonts).
            data = b''.join(result.response)
        else:
            data = result.get_data()
        try:
            return {'status': status, 'content_type': content_type,
                    'body': data.decode('utf-8')}
        except UnicodeDecodeError:
            import base64
            return {'status': status, 'content_type': content_type,
                    'body': base64.b64encode(data).decode('ascii'),
                    'body_base64': True}

    if isinstance(result, (dict, list)):
        return {'status': status,
                'content_type': headers.get('Content-Type', 'application/json'),
                'body': json.dumps(result)}
    if isinstance(result, bytes):
        return {'status': status,
                'content_type': headers.get('Content-Type', 'application/octet-stream'),
                'body': result.decode('utf-8', 'replace')}
    if result is None:
        result = ''
    if not isinstance(result, str):
        result = str(result)
    return {'status': status,
            'content_type': headers.get('Content-Type', 'text/html; charset=utf-8'),
            'body': result}

class Obj:
    def __init__(self, data):
        object.__setattr__(self, '_data', data)

    def __getattr__(self, name):
        data = object.__getattribute__(self, '_data')
        if name in data:
            return wrap(data[name])
        if name == 'id':
            for key in ('id', 'pilot_id', 'heat_id', 'race_id', 'slot_id',
                        'class_id', 'format_id', 'frequencyset_id'):
                if key in data:
                    return data[key]
        raise AttributeError(name)

    def __getitem__(self, key):
        return wrap(object.__getattribute__(self, '_data')[key])

    # Dict-like access: snapshot payloads and event args are read with .get(),
    # keys() and `in` as often as with attribute access.
    def get(self, key, default=None):
        return wrap(object.__getattribute__(self, '_data').get(key, default))

    def keys(self):
        return object.__getattribute__(self, '_data').keys()

    def items(self):
        return ((key, wrap(value))
                for key, value in object.__getattribute__(self, '_data').items())

    def __contains__(self, key):
        return key in object.__getattribute__(self, '_data')

    def __iter__(self):
        return iter(object.__getattribute__(self, '_data'))

    def __repr__(self):
        return 'Obj(%r)' % (object.__getattribute__(self, '_data'),)

def plain(value):
    """Strip Obj wrappers before a payload goes over the wire."""
    if isinstance(value, Obj):
        return plain(object.__getattribute__(value, '_data'))
    if isinstance(value, dict):
        return {key: plain(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [plain(item) for item in value]
    return value


class Rpc:
    '''Shared helper: one server call, unwrapped.'''

    def __init__(self, host):
        self._host = host

    def call(self, obj, method, **args):
        response = self._host.call(RPC_OBJECT, {
            'object': obj, 'method': method, 'args': args})
        if isinstance(response, dict):
            if 'error' in response:
                raise RuntimeError('%s.%s: %s' % (obj, method, response['error']))
            response = response.get('result')
        # Some server arms report failure inside the result envelope.
        if isinstance(response, dict) and set(response) == {'error'}:
            raise RuntimeError('%s.%s: %s' % (obj, method, response['error']))
        return wrap(response)


def field_payload(field):
    '''A UIField as JSON: dataclass attrs, enums reduced to their values.'''
    data = dict(getattr(field, '__dict__', {}))
    for key, value in list(data.items()):
        if hasattr(value, 'value') and not isinstance(value, (str, int, float, bool)):
            data[key] = value.value
        elif isinstance(value, list):
            data[key] = [getattr(v, 'value', v) for v in value]
    return data


class Db(Rpc):
    @property
    def pilots(self):
        return self.call('db', 'pilots') or []

    @property
    def heats(self):
        return self.call('db', 'heats') or []

    def pilot(self, pilot_id):
        return self.call('db', 'pilot', pilot_id=pilot_id)

    def pilot_add(self, name=None, callsign=None, **rest):
        new_id = self.call('db', 'pilot_add', callsign=callsign or name,
                           name=name, **rest)
        return Obj({'id': new_id})

    def pilot_alter(self, pilot_id, attributes=None, **fields):
        return self.call('db', 'pilot_alter', pilot_id=pilot_id,
                         attributes=attributes, **fields)

    def pilot_attribute_value(self, pilot_id, attribute, default=None):
        value = self.call('db', 'pilot_attribute_value', pilot_id=pilot_id,
                          attribute=attribute)
        return default if value is None else value

    def heat_add(self, name=None, **rest):
        return self.call('db', 'heat_add', name=name, **rest)

    def heat_by_id(self, heat_id):
        return self.call('db', 'heat', heat_id=heat_id)

    def heat_alter(self, heat_id, name=None, attributes=None, **fields):
        return self.call('db', 'heat_alter', heat_id=heat_id, name=name,
                         attributes=attributes, **fields)

    def heat_attribute_value(self, heat_id, attribute, default=None):
        value = self.call('db', 'heat_attribute_value', heat_id=heat_id,
                          attribute=attribute)
        return default if value is None else value

    def slots_by_heat(self, heat_id):
        return self.call('db', 'slots_by_heat', heat_id=heat_id) or []

    def pilot_by_id(self, pilot_id):
        return self.call('db', 'pilot', pilot_id=pilot_id)

    def race_by_id(self, race_id):
        return self.call('db', 'race_by_id', race_id=race_id)

    def race_alter(self, race_id, attributes=None, **fields):
        return self.call('db', 'race_alter', race_id=race_id, attributes=attributes,
                         **fields)

    def race_attribute_value(self, race_id, attribute, default=None):
        value = self.call('db', 'race_attribute_value', race_id=race_id,
                          attribute=attribute)
        return default if value is None else value

    def race_ids_by_attribute(self, attribute, value):
        return self.call('db', 'race_ids_by_attribute', attribute=attribute,
                         value=value) or []

    def pilot_ids_by_attribute(self, attribute, value):
        return self.call('db', 'pilot_ids_by_attribute', attribute=attribute,
                         value=value) or []

    def pilotruns_by_race(self, race_id):
        return self.call('db', 'pilotruns_by_race', race_id=race_id) or []

    def laps_by_pilotrun(self, pilotrun_id):
        return self.call('db', 'laps_by_pilotrun', pilotrun_id=pilotrun_id) or []

    def pilotrun_alter(self, pilotrun_id, enter_at=None, exit_at=None, laps=None):
        return self.call('db', 'pilotrun_alter', pilotrun_id=pilotrun_id,
                         enter_at=enter_at, exit_at=exit_at, laps=laps)

    def pilot_by_id(self, pilot_id):
        return self.call('db', 'pilot', pilot_id=pilot_id)

    def race_by_id(self, race_id):
        return self.call('db', 'race_by_id', race_id=race_id)

    def race_alter(self, race_id, attributes=None, **fields):
        return self.call('db', 'race_alter', race_id=race_id, attributes=attributes,
                         **fields)

    def race_attribute_value(self, race_id, attribute, default=None):
        value = self.call('db', 'race_attribute_value', race_id=race_id,
                          attribute=attribute)
        return default if value is None else value

    def race_ids_by_attribute(self, attribute, value):
        return self.call('db', 'race_ids_by_attribute', attribute=attribute,
                         value=value) or []

    def pilot_ids_by_attribute(self, attribute, value):
        return self.call('db', 'pilot_ids_by_attribute', attribute=attribute,
                         value=value) or []

    def pilotruns_by_race(self, race_id):
        return self.call('db', 'pilotruns_by_race', race_id=race_id) or []

    def laps_by_pilotrun(self, pilotrun_id):
        return self.call('db', 'laps_by_pilotrun', pilotrun_id=pilotrun_id) or []

    def pilotrun_alter(self, pilotrun_id, enter_at=None, exit_at=None, laps=None):
        return self.call('db', 'pilotrun_alter', pilotrun_id=pilotrun_id,
                         enter_at=enter_at, exit_at=exit_at, laps=laps)

    @property
    def heat_attributes(self):
        return self._host.fields.get('heat', [])

    @property
    def pilot_attributes(self):
        return self._host.fields.get('pilot', [])

    @property
    def race_attributes(self):
        return self._host.fields.get('race', [])

    @property
    def raceclass_attributes(self):
        return self._host.fields.get('raceclass', [])

    @property
    def raceformat_attributes(self):
        return self._host.fields.get('raceformat', [])

    @property
    def slots(self):
        return self.call('db', 'slots') or []

    def saved_lap_splits_by_pilotrun(self, pilotrun_id):
        return self.call('db', 'saved_lap_splits_by_pilotrun',
                         pilotrun_id=pilotrun_id) or []

    def raceclass_ranking(self, class_id):
        return self.call('db', 'raceclass_ranking', class_id=class_id)

    def raceclass_results(self, class_id):
        return self.call('db', 'raceclass_results', class_id=class_id)

    def pilotrun_auto_adjust(self, pilotrun_id):
        return self.call('db', 'pilotrun_auto_adjust', pilotrun_id=pilotrun_id)

    def pilotrun_auto_adjust_undo(self, pilotrun_id):
        return self.call('db', 'pilotrun_auto_adjust_undo', pilotrun_id=pilotrun_id)

    def pilotrun_suggest_calibration(self, pilotrun_id):
        return self.call('db', 'pilotrun_suggest_calibration',
                         pilotrun_id=pilotrun_id)

    def pilotrun_add(self, race_id, node_index=0, pilot_id=0, **kwargs):
        return self.call('db', 'pilotrun_add', race_id=race_id,
                         node_index=node_index, pilot_id=pilot_id, **kwargs)

    @property
    def saved_lap_splits(self):
        return self.call('db', 'saved_lap_splits') or []

    def frequencyset_add(self, name=None, **kwargs):
        return self.call('db', 'frequencyset_add', name=name, **kwargs)

    def frequencyset_delete(self, frequencyset_id):
        return self.call('db', 'frequencyset_delete', frequencyset_id=frequencyset_id)

    def frequencyset_duplicate(self, frequencyset_id):
        return self.call('db', 'frequencyset_duplicate', frequencyset_id=frequencyset_id)

    def frequencysets_reset(self):
        return self.call('db', 'frequencysets_reset')

    def raceclass_add(self, name=None, format_id=1, **kwargs):
        return self.call('db', 'raceclass_add', name=name, format_id=format_id, **kwargs)

    def raceclass_alter(self, class_id, name=None, **kwargs):
        return self.call('db', 'raceclass_alter', class_id=class_id, name=name, **kwargs)

    def raceclass_delete(self, class_id):
        return self.call('db', 'raceclass_delete', class_id=class_id)

    def raceclass_duplicate(self, class_id):
        return self.call('db', 'raceclass_duplicate', class_id=class_id)

    def raceformat_add(self, name=None, source_format_id=1, **kwargs):
        return self.call('db', 'raceformat_add', name=name,
                         source_format_id=source_format_id, **kwargs)

    def raceformat_alter(self, format_id, name=None, **kwargs):
        return self.call('db', 'raceformat_alter', format_id=format_id,
                         name=name, **kwargs)

    def raceformat_delete(self, format_id):
        return self.call('db', 'raceformat_delete', format_id=format_id)

    def raceformat_duplicate(self, format_id):
        return self.call('db', 'raceformat_duplicate', format_id=format_id)

    def race_add(self, heat_id, round_id=1, class_id=None, format_id=None, **kwargs):
        return self.call('db', 'race_add', heat_id=heat_id, round_id=round_id,
                         class_id=class_id, format_id=format_id, **kwargs)

    def deleted_race_conflicts(self, race_id=None):
        return self.call('db', 'deleted_race_conflicts', race_id=race_id) or []

    def race_restore(self, race_id):
        return self.call('db', 'race_restore', race_id=race_id)

    def open_readonly(self):
        raise NotImplementedError(
            'rhapi.db.open_readonly returns a live DB handle; the Rust server '
            'owns its SQLite connection and cannot hand one to a plugin')

    def deleted_race_data(self, race_id):
        return self.call('db', 'deleted_race_data', race_id=race_id)

    def race_by_heat_round(self, heat_id, round_id):
        return self.call('db', 'race_by_heat_round', heat_id=heat_id, round_id=round_id)

    def race_results(self, race_id):
        return self.call('db', 'race_results', race_id=race_id)

    def heat_results(self, heat_id):
        return self.call('db', 'heat_results', race_id=heat_id)

    def race_results_rebuild(self, race_id=None):
        return self.call('db', 'race_results_rebuild', race_id=race_id)

    def event_results(self, *args, **kwargs):
        return self.call('db', 'event_results')

    def races_clear(self):
        return self.call('db', 'races_clear')

    def deleted_race_by_id(self, race_id):
        return self.call('db', 'deleted_race_by_id', race_id=race_id)

    def raceclass_by_id(self, class_id):
        return self.call('db', 'raceclass_by_id', class_id=class_id)

    def raceformat_by_id(self, format_id):
        return self.call('db', 'raceformat_by_id', format_id=format_id)

    def frequencyset_by_id(self, frequencyset_id):
        return self.call('db', 'frequencyset_by_id',
                         frequencyset_id=frequencyset_id)

    def raceclass_attribute_value(self, class_id, attribute, default=None):
        value = self.call('db', 'raceclass_attribute_value',
                          raceclass_id=class_id, attribute=attribute)
        return default if value is None else value

    def raceformat_attribute_value(self, format_id, attribute, default=None):
        value = self.call('db', 'raceformat_attribute_value',
                          raceformat_id=format_id, attribute=attribute)
        return default if value is None else value

    def heats_by_class(self, class_id):
        return self.call('db', 'heats_by_class', class_id=class_id) or []

    def heats_reset(self):
        return self.call('db', 'heats_reset')

    def pilots_reset(self):
        return self.call('db', 'pilots_reset')

    def raceclasses_reset(self):
        return self.call('db', 'raceclasses_reset')

    def raceformats_reset(self):
        return self.call('db', 'raceformats_reset')

    def reset_all(self):
        return self.call('db', 'reset_all')

    def pilotrun_by_id(self, pilotrun_id):
        return self.call('db', 'pilotrun_by_id', pilotrun_id=pilotrun_id)

    def pilotrun_history(self, pilotrun_id):
        return self.call('db', 'pilotrun_history', pilotrun_id=pilotrun_id)

    def lap_splits(self, lap_id):
        return self.call('db', 'lap_splits', lap_id=lap_id) or []

    def saved_lap_splits_by_race(self, race_id):
        return self.call('db', 'saved_lap_splits_by_race', race_id=race_id) or []

    @property
    def options(self):
        return self.call('db', 'options') or []

    @property
    def races(self):
        return self.call('db', 'races') or []

    @property
    def raceclasses(self):
        return self.call('db', 'raceclasses') or []

    @property
    def raceformats(self):
        return self.call('db', 'raceformats') or []

    @property
    def frequencysets(self):
        return self.call('db', 'frequencysets') or []

    @property
    def laps(self):
        return self.call('db', 'laps') or []

    @property
    def pilotruns(self):
        return self.call('db', 'pilotruns') or []

    @property
    def deleted_races(self):
        return self.call('db', 'deleted_races') or []

    @property
    def backup_dir(self):
        return self.call('db', 'backup_dir')

    @property
    def backup_list(self):
        return self.call('db', 'backup_list') or []

    def pilot_delete(self, pilot_id):
        return self.call('db', 'pilot_delete', pilot_id=pilot_id)

    def heat_duplicate(self, heat_id):
        return self.call('db', 'heat_duplicate', heat_id=heat_id)

    def heat_max_round(self, heat_id):
        return self.call('db', 'heat_max_round', heat_id=heat_id)

    def heat_ids_by_attribute(self, attribute, value):
        return self.call('db', 'heat_ids_by_attribute', attribute=attribute,
                         value=value) or []

    def raceclass_ids_by_attribute(self, attribute, value):
        return self.call('db', 'raceclass_ids_by_attribute', attribute=attribute,
                         value=value) or []

    def raceformat_ids_by_attribute(self, attribute, value):
        return self.call('db', 'raceformat_ids_by_attribute', attribute=attribute,
                         value=value) or []

    def race_delete(self, race_id):
        return self.call('db', 'race_delete', race_id=race_id)

    def races_by_heat(self, heat_id):
        return self.call('db', 'races_by_heat', heat_id=heat_id) or []

    def races_by_raceclass(self, class_id):
        return self.call('db', 'races_by_raceclass', class_id=class_id) or []

    def slot_alter(self, slot_id, pilot=None, **kwargs):
        return self.call('db', 'slot_alter', slot_id=slot_id, pilot=pilot, **kwargs)

    def options_reset(self):
        return self.call('db', 'options_reset')

    def heat_delete(self, heat_id):
        return self.call('db', 'heat_delete', heat_id=heat_id)

    def slots_alter_fast(self, slots):
        return self.call('db', 'slots_alter_fast', slots=slots)

    def option(self, key):
        return self.call('db', 'get_option', name=key)

    def option_set(self, key, value):
        return self.call('db', 'set_option', name=key, value=value)

    def frequencyset_alter(self, frequencyset_id, frequencies=None):
        return self.call('db', 'frequencyset_alter',
                         frequencyset_id=frequencyset_id, frequencies=frequencies)


class Race(Rpc):
    @property
    def status(self):
        data = self.call('race', 'status')
        try:
            return data['race_status']
        except Exception:
            return data

    @property
    def heat(self):
        data = self.call('race', 'heat')
        return data

    @heat.setter
    def heat(self, heat_id):
        if hasattr(heat_id, 'id'):
            heat_id = heat_id.id
        self.call('race', 'set_heat', heat_id=heat_id)

    @property
    def frequencyset(self):
        data = self.call('race', 'status')
        try:
            return Obj({'id': data['frequencyset']})
        except Exception:
            return data

    @property
    def any_laps_recorded(self):
        return bool(self.call('race', 'any_laps_recorded'))

    def stage(self, args=None):
        result = self.call('race', 'stage', options=args or {})
        return result

    def stop(self):
        return self.call('race', 'stop')

    def save(self):
        return self.call('race', 'save')

    def clear(self):
        return self.call('race', 'clear')

    def update_colors(self):
        return self.call('race', 'update_colors')

    # ---- the rest of RaceAPI, served by one snapshot call ----
    def _snap(self):
        return self.call('race', 'snapshot') or {}

    @property
    def pilots(self):
        return self._snap().get('pilots')

    @property
    def teams(self):
        return self._snap().get('teams')

    @property
    def slots(self):
        return self._snap().get('slots')

    @property
    def seat_colors(self):
        return self._snap().get('seat_colors')

    @property
    def round(self):
        return self._snap().get('next_round')

    @property
    def raceformat(self):
        return Obj({'id': self._snap().get('format_id')})

    @property
    def status_message(self):
        return self._snap().get('status_message')

    @property
    def phonetic_status_msg(self):
        return self._snap().get('phonetic_status_msg')

    @property
    def race_time_sec(self):
        return self._snap().get('race_time_sec')

    @property
    def unlimited_time(self):
        return self._snap().get('unlimited_time')

    @property
    def win_status(self):
        return self._snap().get('win_status')

    @property
    def stage_time_internal(self):
        return self._snap().get('stage_time_internal')

    @property
    def start_time(self):
        return self._snap().get('start_time')

    @property
    def start_time_internal(self):
        return self._snap().get('start_time_internal')

    @property
    def end_time_internal(self):
        return self._snap().get('end_time_internal')

    @property
    def seats_finished(self):
        return self._snap().get('seats_finished')

    @property
    def laps(self):
        return self._snap().get('laps')

    @property
    def laps_raw(self):
        return self._snap().get('laps_raw')

    @property
    def laps_active_raw(self):
        return self._snap().get('laps_raw')

    @property
    def scheduled(self):
        return self._snap().get('scheduled')

    @property
    def results(self):
        return self._snap().get('results')

    @property
    def coop_results(self):
        return self._snap().get('coop_results')

    @property
    def team_results(self):
        return self._snap().get('team_results')

    @property
    def last_race(self):
        return self._snap().get('last_race')

    @property
    def race_leader_pilot_id(self):
        return self._snap().get('race_leader_pilot_id')

    @property
    def race_leader_lap(self):
        return self._snap().get('race_leader_lap')

    @property
    def race_winner_pilot_id(self):
        return self._snap().get('race_winner_pilot_id')

    @property
    def race_winner_name(self):
        return self._snap().get('race_winner_name')

    @property
    def race_winner_phonetic(self):
        return self._snap().get('race_winner_phonetic')

    @property
    def race_winner_lap_id(self):
        return self._snap().get('race_winner_lap_id')

    @property
    def prev_race_winner_pilot_id(self):
        return self._snap().get('prev_race_winner_pilot_id')

    @property
    def prev_race_winner_name(self):
        return self._snap().get('prev_race_winner_name')

    @property
    def prev_race_winner_phonetic(self):
        return self._snap().get('prev_race_winner_phonetic')

    def lap_add(self, seat, timestamp, peak=None):
        return self.call('race', 'lap_add', seat=seat, timestamp=timestamp, peak=peak)

    def lap_insert(self, seat, lap_time_stamp, source=4, peak=None):
        return self.call('race', 'lap_insert', seat=seat,
                         lap_time_stamp=lap_time_stamp, source=source, peak=peak)

    def laps_replace(self, seat, laps):
        return self.call('race', 'laps_replace', seat=seat, laps=laps)

    def seat_auto_adjust(self, seat):
        return self.call('race', 'seat_auto_adjust', seat=seat)

    def seat_auto_adjust_undo(self, seat):
        return self.call('race', 'seat_auto_adjust_undo', seat=seat)

    def seat_suggest_calibration(self, seat):
        return self.call('race', 'seat_suggest_calibration', seat=seat)

    def schedule(self, time_value=0, **kwargs):
        return self.call('race', 'schedule', time=time_value)

    @property
    def laps(self):
        return self.call('race', 'laps')

    @property
    def laps(self):
        return self.call('race', 'laps')


class Fields(Rpc):
    def _remember(self, kind, field):
        self._host.fields.setdefault(kind, []).append(field_payload(field))

    @property
    def options(self):
        return self._host.fields.get('options', [])

    @property
    def heat_attributes(self):
        return self._host.fields.get('heat', [])

    @property
    def pilot_attributes(self):
        return self._host.fields.get('pilot', [])

    @property
    def race_attributes(self):
        return self._host.fields.get('race', [])

    @property
    def raceclass_attributes(self):
        return self._host.fields.get('raceclass', [])

    @property
    def raceformat_attributes(self):
        return self._host.fields.get('raceformat', [])

    def register_race_attribute(self, field):
        self._remember('race', field)
        return self.call('fields', 'register_race_attribute',
                         field=field_payload(field))

    def register_pilot_attribute(self, field):
        self._remember('pilot', field)
        return self.call('fields', 'register_pilot_attribute',
                         field=field_payload(field))

    def register_option(self, field, panel):
        self._remember('options', field)
        return self.call('fields', 'register_option',
                         field=field_payload(field), panel=panel)

    def register_heat_attribute(self, field):
        return self.call('fields', 'register_heat_attribute', field=field_payload(field))

    def register_raceclass_attribute(self, field):
        return self.call('fields', 'register_raceclass_attribute',
                         field=field_payload(field))

    def register_raceformat_attribute(self, field):
        return self.call('fields', 'register_raceformat_attribute',
                         field=field_payload(field))

    def register_function_binding(self, field, getter_fn, panel=None, args=None):
        return True


class Config(Rpc):
    '''rhapi.config.get/set -- the server stores these as db options.'''

    def get(self, section, key, default=None):
        try:
            value = self.call('config', 'get', section=section, key=key)
        except Exception:
            return default
        if value in (None, ''):
            return default
        return value

    def set(self, section, key, value):
        return self.call('config', 'set', section=section, key=key, value=value)

    def get_item(self, section, key, default=None):
        return self.get(section, key, default)

    def set_item(self, section, key, value):
        return self.set(section, key, value)

    @property
    def get_all(self):
        return self.call('config', 'all') or {}

    @property
    def config(self):
        return self.call('config', 'all') or {}

    def register_section(self, section, items=None):
        return self.call('ui', 'register_config_section', section=section, items=items)


class Utils:
    '''RHAPI.utils: delegates to the real RHUtils/frequencies, shipped in
    rh_compat, so formatting and band lookups cannot drift.'''

    def __init__(self):
        import RHUtils as rh_utils
        import frequencies
        self._u = rh_utils
        self._f = frequencies

    def format_time_to_str(self, millis, time_format=None):
        return self._u.format_time_to_str(
            millis, timeformat=time_format or '{m}:{s}.{d}')

    def format_split_time_to_str(self, millis, time_format=None):
        return self._u.format_split_time_to_str(
            millis, timeformat=time_format or '{m}:{s}.{d}')

    def format_phonetic_time_to_str(self, millis, time_format=None):
        return self._u.format_phonetic_time_to_str(
            millis, timeformat=time_format or '{m} {s}.{d}')

    def generate_unique_name(self, desired_name, other_names):
        return self._u.uniqueName(desired_name, other_names)

    def generate_unique_name_from_base(self, base_name, other_names):
        return self._u.unique_name_from_base(base_name, other_names)

    def color_hsl_to_hexstring(self, h, s, l):
        return self._u.hslToHex(h, s, l)

    def color_hexstring_to_int(self, hex_color):
        return self._u.hexToColor(hex_color)

    @property
    def band_channel_frequencies(self):
        return dict(self._f.BAND_CHANNEL_FREQUENCIES)

    def band_channel_for_frequency(self, frequency):
        return self._f.band_channel_for(frequency)

    def frequency_for_band_channel(self, band, channel):
        return self._f.frequency_for(band, channel)


class ServerInfo(dict):
    '''server.info: rhapi_version, the server's release version, api_version.

    The version comes from the server so it cannot drift from RELEASE_VERSION;
    a bump in state.rs is the only place it is written.
    '''

    def __init__(self, host=None):
        super().__init__(rhapi_version=(1, 7), version='unknown', api_version=1)
        self._host = host
        self.refresh()

    def refresh(self):
        '''Pull release_version from the server.

        Rhapi is built before socket.io connects, so the first attempt from
        __init__ fails; Bridge calls refresh() once connected.
        '''
        if self._host is None:
            return
        try:
            raw = self._host.call(RPC_OBJECT, {
                'object': 'server', 'method': 'info', 'args': {}})
            data = (object.__getattribute__(raw, '_data')
                    if isinstance(raw, Obj) else (raw or {}))
            if isinstance(data, dict) and data.get('result'):
                data = data['result']
            version = data.get('release_version') if isinstance(data, dict) else None
            if version:
                self['version'] = version
        except Exception:  # noqa: BLE001 - info must never break a plugin
            pass


class UI(Rpc):
    def __init__(self, host):
        super().__init__(host)
        self._host = host

    def socket_listen(self, event, handler):
        '''Register a handler for a client-emitted socket event.'''
        self._host.listeners.setdefault(event, []).append(handler)
        return True

    def socket_broadcast(self, event, payload):
        return self.call('ui', 'socket_broadcast', event=event, payload=payload)

    def broadcast_pilots(self):
        return self.call('ui', 'broadcast', what='pilots')

    def broadcast_heats(self):
        return self.call('ui', 'broadcast', what='heats')

    def broadcast_current_heat(self):
        return self.call('ui', 'broadcast', what='current_heat')

    def broadcast_ui(self, panel=None):
        return self.call('ui', 'broadcast', what='ui', panel=panel)

    def register_panel(self, domain, name, location, order=0, open=False):
        self._host.panels[domain] = {'name': name, 'page': location,
                                     'order': order, 'open': open}
        return self.call('ui', 'register_panel', domain=domain, name=name,
                         location=location, order=order)

    def message_speak(self, message):
        return self.call('ui', 'message_speak', message=message)

    def message_notify(self, message):
        return self.call('ui', 'message_notify', message=message)

    def message_alert(self, message):
        return self.call('ui', 'message_alert', message=message)

    def clear_messages(self):
        return self.call('ui', 'clear_messages')

    def register_config_section(self, section, items=None):
        return self.call('ui', 'register_config_section', section=section,
                         items=items)

    @property
    def panels(self):
        return dict(self._host.panels)

    def broadcast_frequencies(self):
        return self.call('ui', 'broadcast', what='frequencies')

    def broadcast_frequencyset(self):
        return self.call('ui', 'broadcast', what='frequencyset')

    def broadcast_race_status(self):
        return self.call('ui', 'broadcast', what='race_status')

    def broadcast_raceclasses(self):
        return self.call('ui', 'broadcast', what='raceclasses')

    def broadcast_raceformats(self):
        return self.call('ui', 'broadcast', what='raceformats')

    def register_markdown(self, panel=None, name=None, desc=None, markdown=None):
        '''Matches RHAPI: register_markdown(panel, name, desc).

        A dict as the first argument is also accepted (older bridge form).
        '''
        if isinstance(panel, dict) and markdown is None:
            markdown, panel = panel, None
        if markdown is not None:
            name = name or markdown.get('name')
            desc = desc if desc is not None else markdown.get('desc')
            panel = panel or markdown.get('panel')
        return self.call('ui', 'register_markdown', panel=panel, name=name, desc=desc)

    def register_quickbutton(self, panel=None, name=None, label=None, function=None,
                             args=None, requires_auth=False, client_context=False,
                             button=None):
        '''Matches RHAPI: register_quickbutton(panel, name, label, function).

        The callable stays here: the server only stores the button, and a click
        arrives as the button's name, which is bound as a local listener so the
        plugin's function runs and its return value becomes the socket ack.
        '''
        if isinstance(panel, dict) and button is None:
            button, panel = panel, None
        if button is not None:
            name = name or button.get('name')
            label = label or button.get('label')
            function = function or button.get('function')
            panel = panel or button.get('panel')
        if callable(function) and name:
            self._host.listeners.setdefault(name, []).append(function)
        return self.call('ui', 'register_quickbutton', panel=panel,
                         button={'name': name, 'label': label or name})

    def blueprint_add(self, blueprint):
        '''Registers a plugin's HTTP routes with the server.

        With a real Flask available (the common case) the blueprint is mounted on
        a small app here, so url_prefix, converters, templates and static folders
        all behave as they do under the Python server. Without Flask, the shim's
        recorded routes are used and responses are mapped by hand.
        '''
        domain = getattr(self._host, '_loading_plugin', None)
        name = getattr(blueprint, 'name', '?')
        app = self._host.flask_app()
        registered = 0

        if app is not None:
            try:
                before = {rule.endpoint for rule in app.url_map.iter_rules()}
                app.register_blueprint(blueprint)
                for rule in app.url_map.iter_rules():
                    if rule.endpoint in before:
                        continue
                    if not rule.endpoint.startswith(name + '.'):
                        continue
                    methods = sorted(method for method in rule.methods
                                     if method not in ('HEAD', 'OPTIONS'))
                    self._host.routes.append({
                        'domain': domain, 'rule': rule.rule, 'methods': methods,
                        'endpoint': rule.endpoint, 'view': None})
                    self.call('plugin', 'register_route', domain=domain,
                              rule=rule.rule, methods=methods)
                    registered += 1
            except Exception as ex:  # noqa: BLE001 - a route is not worth a load failure
                print('[bridge] could not mount blueprint {}: {}'.format(name, ex),
                      file=sys.stderr)
        else:
            for rule, options, view in getattr(blueprint, 'routes', None) or []:
                methods = options.get('methods') or ['GET']
                methods = sorted({str(method).upper() for method in methods})
                self._host.routes.append({'domain': domain, 'rule': rule,
                                          'methods': methods, 'endpoint': None,
                                          'view': view})
                try:
                    self.call('plugin', 'register_route', domain=domain, rule=rule,
                              methods=methods)
                    registered += 1
                except Exception as ex:  # noqa: BLE001
                    print('[bridge] could not register route {}: {}'.format(rule, ex),
                          file=sys.stderr)

        print('[bridge] blueprint {}: {} route(s)'.format(name, registered))
        return True

    def register_script(self, path):
        return self.call('ui', 'register_script', path=path)

    def register_stylesheet(self, path):
        return self.call('ui', 'register_stylesheet', path=path)

    def update_field(self, field):
        return self.call('ui', 'update_field', field=field)

    def update_markdown(self, markdown):
        return self.call('ui', 'update_markdown', markdown=markdown)

    def socket_send(self, event, payload):
        return self.call('ui', 'socket_send', event=event, payload=payload)

    def socket_send_to(self, sid, event, payload):
        return self.call('ui', 'socket_send_to', sid=sid, event=event, payload=payload)



class Events:
    '''Event registration: plugins register hooks by event name; server
    broadcasts are dispatched to them here.'''

    def __init__(self, host):
        self._host = host
        self._handlers = {}

    def on(self, event, handler_fn, default_args=None, priority=None,
           unique=False, name=None):
        '''Matches RHAPI.EventsAPI.on, including default_args.

        Python merges a handler's default_args into the args dict at emit time;
        the entry keeps them so dispatch can do the same.
        '''
        entry = {'handler': handler_fn, 'default_args': default_args or {}}
        handlers = self._handlers.setdefault(event, [])
        if unique:
            handlers[:] = [existing for existing in handlers
                           if existing['handler'] is not handler_fn]
        handlers.append(entry)
        return handler_fn

    def off(self, event, handler_fn=None):
        if handler_fn is None:
            self._handlers.pop(event, None)
            return True
        handlers = self._handlers.get(event, [])
        handlers[:] = [entry for entry in handlers
                       if entry['handler'] is not handler_fn]
        return True

    def trigger(self, event, args=None):
        '''Python triggers the server's event bus; plugins see the same hooks,
        so dispatch locally (the server does not need a round trip).'''
        self.dispatch(event, args or {})
        return True


    def dispatch(self, event, args):
        # Python hands plugins a dict of args with objects inside it
        # (args['node_index'], args['lap'].lap_time_stamp), so keep the dict
        # dict-like and only wrap the values that are records.
        if isinstance(args, dict):
            args = {key: (wrap(value) if isinstance(value, dict) else value)
                    for key, value in args.items()}
        handlers = self._handlers.get(event, [])
        for entry in handlers:
            try:
                handler_args = args
                defaults = entry.get('default_args') or {}
                if defaults and isinstance(args, dict):
                    handler_args = dict(defaults)
                    handler_args.update(args)
                entry['handler'](handler_args)
            except Exception:
                print('[bridge] handler error for {}:'.format(event), file=sys.stderr)
                import traceback
                traceback.print_exc()
            finally:
                self._loading_plugin = None

class Filters:
    '''RHAPI filters: callables applied to a payload before it is emitted.'''

    def __init__(self):
        self._filters = []

    def add(self, fn):
        self._filters.append(fn)
        return True

    def remove(self, fn):
        if fn in self._filters:
            self._filters.remove(fn)
        return True

    def run(self, data, args=None):
        for fn in list(self._filters):
            try:
                data = fn(data, args or {})
            except Exception:
                import traceback
                traceback.print_exc()
        return data


class Sensors:
    '''The Rust build has no sensors (BME280 dropped), so the API is empty.'''

    @property
    def sensor_names(self):
        return []

    @property
    def sensors_dict(self):
        return {}

    @property
    def sensor_objs(self):
        return []

    def sensor_obj(self, name):
        return None


class Server:
    def __init__(self, host, server_info):
        self._host = host
        self._info = server_info
        self._start_monotonic = time.monotonic()
        self._start_epoch = time.time()

    @property
    def data_dir(self):
        return self.call('server', 'data_dir')

    @property
    def program_dir(self):
        return self.call('server', 'program_dir')

    @property
    def info(self):
        return dict(self._info)

    @property
    def plugins(self):
        return list(self._host._plugins)

    @property
    def seat_color_defaults(self):
        return self.call('server', 'seat_color_defaults')

    @property
    def program_start_mtonic(self):
        return self._start_monotonic

    @property
    def program_start_epoch_time(self):
        return self._start_epoch

    @property
    def program_start_time_formatted(self):
        return time.strftime('%H:%M:%S', time.localtime(self._start_epoch))

    @property
    def program_start_epoch_formatted(self):
        return time.strftime('%Y-%m-%d %H:%M:%S', time.localtime(self._start_epoch))

    @property
    def mtonic_to_epoch_millis_offset(self):
        return (time.time() - time.monotonic()) * 1000.0

    def monotonic_to_epoch_millis(self, value):
        return value * 1000.0 + self.mtonic_to_epoch_millis_offset

    def epoch_millis_to_monotonic(self, value):
        return (value - self.mtonic_to_epoch_millis_offset) / 1000.0

    def enable_heartbeat_event(self, enabled=True):
        return True

    def set_min_mocks(self, count):
        return True

    def set_restart_required(self, required=True):
        return True

    def call(self, obj, method, **args):
        return wrap(self._host.call(RPC_OBJECT,
                                    {'object': obj, 'method': method, 'args': args}))


class Language:
    def __init__(self, host):
        self._host = host

    @property
    def languages(self):
        return self.call('language', 'languages') or []

    @property
    def dictionary(self):
        return {}

    def __call__(self, text):
        return text

    def call(self, obj, method, **args):
        return wrap(self._host.call(RPC_OBJECT,
                                    {'object': obj, 'method': method, 'args': args}))


class Led:
    def __init__(self, host):
        self._host = host

    def _call(self, method, **args):
        return wrap(self._host.call(RPC_OBJECT,
                                    {'object': 'led', 'method': method, 'args': args}))

    @property
    def enabled(self):
        return bool(self._call('enabled'))

    @property
    def effects(self):
        return self._call('effects') or {}

    def activate_effect(self, effect, args=None):
        return self._call('activate_effect', effect=effect, args=args or {})

    def effect_set(self, event, effect):
        return self._call('effect_set', event=event, effect=effect)

    def effect_by_event(self, event):
        return self._call('effect_by_event', event=event)

    def display_color(self, color, **kwargs):
        return self._call('display_color', color=color, **kwargs)

    def clear(self):
        return self._call('clear')


class VRxControl:
    def __init__(self, host):
        self._host = host

    def _call(self, method, **args):
        return wrap(self._host.call(RPC_OBJECT,
                                    {'object': 'vrx', 'method': method, 'args': args}))

    @property
    def enabled(self):
        return bool(self._call('enabled'))

    @property
    def status(self):
        return self._call('status')

    @property
    def devices(self):
        return self._call('devices') or {}

    def devices_by_pilot(self, seat=0, pilot_id=None):
        return self._call('devices_by_pilot', seat=seat, pilot_id=pilot_id) or []

    def kill(self):
        return self._call('kill')


class Registry:
    """classrank / points: a read-only list of the implementations the server
    offers (plugins pick one by name)."""

    def __init__(self, host, obj):
        self._host = host
        self._obj = obj

    @property
    def methods(self):
        return wrap(self._host.call(RPC_OBJECT, {
            'object': self._obj, 'method': 'methods', 'args': {}})) or []


class HeatGen:
    def __init__(self, host):
        self._host = host

    @property
    def generators(self):
        return wrap(self._host.call(RPC_OBJECT, {
            'object': 'heatgen', 'method': 'generators', 'args': {}})) or []

    def generate(self, generator, params=None):
        raise NotImplementedError(
            'rhapi.heatgen.generate is not bridged: run heat generation from the UI')


class Io:
    def __init__(self, host):
        self._host = host

    @property
    def exporters(self):
        return wrap(self._host.call(RPC_OBJECT, {
            'object': 'io', 'method': 'exporters', 'args': {}})) or []

    @property
    def importers(self):
        return wrap(self._host.call(RPC_OBJECT, {
            'object': 'io', 'method': 'importers', 'args': {}})) or []

    def run_export(self, *args, **kwargs):
        raise NotImplementedError('rhapi.io.run_export is not bridged')

    def run_import(self, *args, **kwargs):
        raise NotImplementedError('rhapi.io.run_import is not bridged')


class Interface:
    """rhapi.interface: the seat/hardware surface. The Rust server owns node
    providers, so this is a read-only view plus accepted registrations."""

    def __init__(self, host):
        self._host = host

    def _call(self, method, **args):
        return wrap(self._host.call(RPC_OBJECT, {
            'object': 'interface', 'method': method, 'args': args}))

    @property
    def seats(self):
        return self._call('seats')

    @property
    def mock_signals(self):
        return self._call('mock_signals') or []

    def is_mock_only(self):
        return bool(self._call('is_mock_only'))

    def seat_provider_type(self, seat):
        return self._call('seat_provider_type', seat=seat)

    def add(self, seat, provider):
        return self._call('add', seat=seat, provider=provider)

    def register_mock_signal(self, name, fn):
        return self._call('register_mock_signal', name=name)

    def seat_crossing_end(self, seat, crossing):
        return self._call('seat_crossing_end', seat=seat, crossing=crossing)

    def seat_enter_at_set(self, seat, level):
        return self._call('seat_enter_at_set', seat=seat, level=level)

    def seat_exit_at_set(self, seat, level):
        return self._call('seat_exit_at_set', seat=seat, level=level)


class EventResults:
    """rhapi.eventresults: results across the event (server-side results are
    rebuilt from saved races, so this is a read-only RPC)."""

    def __init__(self, host):
        self._host = host

    @property
    def results(self):
        return wrap(self._host.call(RPC_OBJECT, {
            'object': 'eventresults', 'method': 'results', 'args': {}}))


class Rhapi:
    '''The object surface handed to plugins.'''

    def __init__(self, host):
        self.db = Db(host)
        self.race = Race(host)
        self.pilot = Db(host)
        self.ui = UI(host)
        self.events = Events(host)
        self.fields = Fields(host)
        self.config = Config(host)
        self.utils = Utils()
        self.server_info = ServerInfo(host)
        self.server = Server(host, self.server_info)
        self.language = Language(host)
        self.led = Led(host)
        self.vrxcontrol = VRxControl(host)
        self.classrank = Registry(host, 'classrank')
        self.points = Registry(host, 'points')
        self.heatgen = HeatGen(host)
        self.io = Io(host)
        self.interface = Interface(host)
        self.eventresults = EventResults(host)
        self.filters = Filters()
        self.sensors = Sensors()
        # rhapi.__ is RHAPI's translation helper; English is the identity.
        setattr(self, '__', lambda text: text)
        self._host = host


class Bridge:
    def __init__(self, server_url):
        self.sio = socketio.Client()
        self._call_lock = threading.Lock()
        self._call_id = 0
        self._acks = {}
        self._cond = threading.Condition()
        self.listeners = {}
        self.panels = {}
        self.fields = {}

        self.sio.on('rhapi_response', self._on_response)
        self.sio.on('rhapi_event', self._on_plugin_event)
        self.sio.on('plugin_invoke', self._on_invoke)
        self.sio.on('*', self._on_any)

        self.rhapi = Rhapi(self)
        self._plugins = []
        self.routes = []
        self._loading_plugin = None
        self._flask_app = None
        # Server -> bridge: run a plugin's HTTP route (see ROUTE_EVENT).
        self.listeners.setdefault(ROUTE_EVENT, []).append(self._handle_route)

        self.sio.connect(server_url, wait_timeout=15)
        # The RPC transport exists only after connect, so the version is read here.
        self.rhapi.server_info.refresh()
        print('[bridge] connected to', server_url)

    def flask_app(self):
        '''The app plugin blueprints are mounted on, or None without Flask.'''
        try:
            import flask
        except ImportError:
            return None
        if not hasattr(flask, 'Flask'):
            return None  # our stand-in, which has no app to mount onto
        if self._flask_app is None:
            self._flask_app = flask.Flask('rhapi_bridge')
        return self._flask_app

    # ---- plugin HTTP routes ----
    def _handle_route(self, args):
        """Runs the plugin view function for a forwarded request."""
        args = args or {}
        method = str(args.get('method', 'GET')).upper()
        path = str(args.get('path', '/'))

        app = self.flask_app()
        if app is not None:
            return self._route_with_flask(app, method, path, args)

        # no Flask: use the shim's recorded rules and the shared request object
        for route in self.routes:
            allowed = route['methods']
            if method not in allowed and not (method == 'HEAD' and 'GET' in allowed):
                continue
            params = _match_rule(route['rule'], path)
            if params is None:
                continue
            previous = _set_request(args, params)
            try:
                result = route['view'](**params)
            except Exception as ex:  # noqa: BLE001 - the plugin's error is the body
                import traceback
                traceback.print_exc()
                return {'status': 500, 'content_type': 'text/plain; charset=utf-8',
                        'body': '{}: {}'.format(type(ex).__name__, ex)}
            finally:
                _restore_request(previous)
            return _normalise_response(result)
        return {'status': 404, 'content_type': 'text/plain; charset=utf-8',
                'body': 'Not Found'}

    def _route_with_flask(self, app, method, path, args):
        """Flask does the matching, so converters hand the view real types."""
        from werkzeug.exceptions import MethodNotAllowed, NotFound
        try:
            endpoint, values = app.url_map.bind('localhost').match(path, method=method)
        except MethodNotAllowed:
            return {'status': 405, 'content_type': 'text/plain; charset=utf-8',
                    'body': 'Method Not Allowed'}
        except NotFound:
            return {'status': 404, 'content_type': 'text/plain; charset=utf-8',
                    'body': 'Not Found'}
        except Exception as ex:  # noqa: BLE001
            return {'status': 400, 'content_type': 'text/plain; charset=utf-8',
                    'body': '{}: {}'.format(type(ex).__name__, ex)}

        view = app.view_functions.get(endpoint)
        if view is None:
            return {'status': 404, 'content_type': 'text/plain; charset=utf-8',
                    'body': 'Not Found'}
        try:
            with app.test_request_context(
                    path=path,
                    method=method,
                    query_string=dict(args.get('query') or {}),
                    data=str(args.get('body') or ''),
                    headers=dict(args.get('headers') or {})):
                return _normalise_response(view(**values))
        except Exception as ex:  # noqa: BLE001 - the plugin's error is the body
            import traceback
            traceback.print_exc()
            return {'status': 500, 'content_type': 'text/plain; charset=utf-8',
                    'body': '{}: {}'.format(type(ex).__name__, ex)}

    def _on_response(self, data):
        with self._cond:
            call_id = (data or {}).get('id')
            if call_id is not None:
                self._acks[call_id] = data or {}
                self._cond.notify_all()

    def _on_invoke(self, data):
        """A client socket event that a plugin listens for, sent by the server
        as plugin_invoke; the handler's return value goes back as the ack."""
        data = data or {}

        def run():
            # Handlers make blocking RPC calls, and this returns on the socket.io
            # reader thread; blocking it would stop the very acks those calls
            # wait for, so each invocation runs on its own thread.
            outcome = self.invoke(data.get('event', ''), data.get('args'))
            if 'result' in outcome:
                result = outcome['result']
            else:
                result = {'error': outcome.get('error', 'plugin error')}
            if data.get('id') is not None:
                self.sio.emit('plugin_result',
                              {'id': data.get('id'), 'result': result})

        threading.Thread(target=run, daemon=True).start()

    def _on_plugin_event(self, data):
        data = data or {}
        # Handlers make blocking RPC calls; dispatching inline would occupy the
        # socket.io reader thread and the acks those calls wait for could never
        # be processed (the whole save path deadlocked this way).
        threading.Thread(
            target=self.rhapi.events.dispatch,
            args=(data.get('event', ''), data.get('args') or {}),
            daemon=True).start()

    def _on_any(self, event, *args):
        args_payload = args[0] if args else {}
        threading.Thread(target=self.rhapi.events.dispatch,
                         args=(event, args_payload), daemon=True).start()

    # ---- bridge -> server ----
    def call(self, event, payload):
        '''Synchronous RPC via socket.io ack.'''
        payload = plain(payload)
        with self._call_lock:
            self._call_id += 1
            call_id = self._call_id
            payload = dict(payload, id=call_id)
        with self._cond:
            self.sio.emit(event, payload, callback=self._ack_callback(call_id))
            deadline = time.monotonic() + RPC_TIMEOUT
            while call_id not in self._acks:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise RuntimeError('RHAPI RPC timeout: {}'.format(
                        payload.get('method')))
                self._cond.wait(remaining)
            return self._acks.pop(call_id)

    def _ack_callback(self, call_id):
        def cb(*args):
            with self._cond:
                self._acks[call_id] = args[0] if args else {}
                self._cond.notify_all()
        return cb

    # ---- plugin dependencies ----
    @staticmethod
    def _dependency_name(spec):
        """'esptool>=4.8.1' -> 'esptool' (the importable module)."""
        return re.split(r'[\[<>=!~; ]', str(spec), 1)[0].strip()

    def _missing_specs(self, specs):
        out = []
        for spec in specs:
            name = self._dependency_name(spec)
            if not name:
                continue
            try:
                if importlib.util.find_spec(name) is not None:
                    continue
            except (ImportError, ValueError):
                pass
            out.append(spec)
        return out

    def _pip_install(self, specs):
        """Install into the interpreter running this bridge.

        Same command the Python server's plugin installer uses, so a plugin that
        declares dependencies behaves the same on either server.
        """
        return subprocess.run([sys.executable, '-m', 'pip', 'install', *specs],
                              capture_output=True, text=True, timeout=300)

    def _ensure_dependencies(self, manifest):
        """None when satisfied, else a message for the plugin list.

        A plugin that cannot import its dependency is more useful reported than
        half-loaded: the message names the exact command to run by hand.
        """
        specs = [str(spec) for spec in (manifest.get('dependencies') or [])
                 if str(spec).strip()]
        if not specs:
            return None

        missing = self._missing_specs(specs)
        if not missing:
            return None

        command = '{} -m pip install {}'.format(sys.executable, ' '.join(missing))
        if os.environ.get('RH_PLUGIN_NO_DEPS'):
            return 'missing dependencies: %s (auto-install disabled)' % command

        print('[bridge] installing dependencies: {}'.format(', '.join(missing)))
        try:
            result = self._pip_install(missing)
        except (OSError, subprocess.SubprocessError) as ex:
            return 'dependencies not installed ({}). Run: {}'.format(ex, command)

        importlib.invalidate_caches()
        if result.returncode != 0:
            detail = [line for line in (result.stderr or result.stdout or '').splitlines()
                      if line.strip()]
            return 'dependencies failed to install: {}. Run: {} (if pip is missing: ' \
                   '{} -m ensurepip)'.format(detail[-1] if detail else 'pip error',
                                             command, sys.executable)

        still = self._missing_specs(specs)
        if still:
            return 'dependencies installed but not importable: {}. Run: {}'.format(
                ', '.join(still), command)
        return None

    # ---- plugin loading ----
    def load_plugins(self, plugins_dir, only=None):
        if not os.path.isdir(plugins_dir):
            print('[bridge] plugins dir not found:', plugins_dir)
            return
        report = []
        for name in sorted(os.listdir(plugins_dir)):
            if only and name not in only:
                continue
            plugin_dir = os.path.join(plugins_dir, name)
            init_py = os.path.join(plugin_dir, '__init__.py')
            if not os.path.isfile(init_py):
                continue
            manifest_path = os.path.join(plugin_dir, 'manifest.json')
            manifest = {}
            if os.path.isfile(manifest_path):
                with open(manifest_path) as fh:
                    manifest = json.load(fh)
            dependency_problem = self._ensure_dependencies(manifest)
            if dependency_problem:
                report.append({'domain': name, 'error': dependency_problem})
                print('[bridge] plugin {} not loaded: {}'.format(name, dependency_problem),
                      file=sys.stderr)
                continue
            spec = importlib.util.spec_from_file_location(
                'rhplugin_' + name, init_py)
            module = importlib.util.module_from_spec(spec)
            sys.modules['rhplugin_' + name] = module
            try:
                self._loading_plugin = name
                spec.loader.exec_module(module)
                if hasattr(module, 'initialize'):
                    plugin = module.initialize(self.rhapi)
                    if plugin is not None and hasattr(plugin, 'initialize'):
                        plugin.initialize(None)
                self._plugins.append(name)
                report.append({'domain': name, 'error': None})
                print('[bridge] loaded plugin: {} ({})'.format(
                    name, manifest.get('name', name)))
            except Exception as ex:
                report.append({'domain': name, 'error': '{}: {}'.format(
                    type(ex).__name__, ex)})
                print('[bridge] failed to load plugin {}: {}'.format(name, ex),
                      file=sys.stderr)
                import traceback
                traceback.print_exc()

        # Tell the server what actually loaded, so the settings page can show
        # "Loaded" or the reason instead of guessing from "the bridge is up".
        try:
            self.rhapi.db.call('plugin', 'report_status', plugins=report)
        except Exception as ex:  # noqa: BLE001 - never fail a load over reporting
            print('[bridge] could not report plugin status: {}'.format(ex),
                  file=sys.stderr)

    # ---- test/control surface (localhost only) ----
    def invoke(self, event, args=None):
        '''Call the handlers a plugin registered for a socket event, as the
        server would when a client emits it.'''
        handlers = self.listeners.get(event, [])
        if not handlers:
            return {'error': 'no listener for ' + event}
        results = []
        for handler in handlers:
            try:
                results.append(handler(args))
            except Exception as ex:
                import traceback
                results.append({'error': '%s: %s' % (type(ex).__name__, ex),
                                'traceback': traceback.format_exc().splitlines()[-14:]})
        return {'result': results[0] if len(results) == 1 else results}


def serve_control(bridge, port):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            length = int(self.headers.get('content-length') or 0)
            body = self.rfile.read(length) if length else b'{}'
            try:
                request = json.loads(body or b'{}')
            except Exception:
                request = {}
            payload = bridge.invoke(request.get('event', ''),
                                    request.get('args'))
            raw = json.dumps(payload, default=str).encode()
            self.send_response(200)
            self.send_header('content-type', 'application/json')
            self.send_header('content-length', str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def do_GET(self):
            raw = json.dumps({
                'plugins': bridge._plugins,
                'listeners': sorted(bridge.listeners),
            }).encode()
            self.send_response(200)
            self.send_header('content-type', 'application/json')
            self.send_header('content-length', str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

    server = ThreadingHTTPServer(('127.0.0.1', port), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    print('[bridge] control endpoint on http://127.0.0.1:%d' % port)
    return server


def main():
    parser = argparse.ArgumentParser(description='RotorHazard plugin bridge')
    parser.add_argument('--server', default='http://127.0.0.1:5000')
    parser.add_argument('--plugins-dir', default='./plugins')
    parser.add_argument('--plugins', default=None,
                        help='comma-separated plugin names (default: all)')
    parser.add_argument('--control-port', type=int, default=0,
                        help='localhost HTTP control surface for tests')
    args = parser.parse_args()

    bridge = Bridge(args.server)
    only = set(args.plugins.split(',')) if args.plugins else None
    bridge.load_plugins(args.plugins_dir, only)

    # Plugins initialise on Evt.STARTUP, which the server fires once at boot;
    # loading a plugin is not enough, its handlers register during startup.
    bridge.rhapi.events.dispatch('startup', None)

    if args.control_port:
        serve_control(bridge, args.control_port)

    # Tell the server which client events the loaded plugins listen for; it
    # registers a forwarding handler for each on every socket connect.
    if bridge.listeners:
        try:
            bridge.call(RPC_OBJECT, {
                'object': 'plugin', 'method': 'listen',
                'args': {'names': sorted(bridge.listeners)}})
            print('[bridge] registered %d socket listener(s)' % len(bridge.listeners))
        except Exception as ex:
            print('[bridge] listener registration failed: %s' % ex, file=sys.stderr)

    print('[bridge] running; {} plugin(s) loaded'.format(len(bridge._plugins)))
    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        print('[bridge] shutting down')
        bridge.sio.disconnect()


if __name__ == '__main__':
    main()
