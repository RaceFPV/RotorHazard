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

RPC_OBJECT = 'rhapi_call'
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
        print("[bridge] pilot_alter(%r)" % (pilot_id,), file=sys.stderr)
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
        self.call('race', 'set_heat', heat_id=heat_id)

    @property
    def frequencyset(self):
        data = self.call('race', 'status')
        try:
            return data['frequencyset']
        except Exception:
            return data

    @property
    def any_laps_recorded(self):
        return bool(self.call('race', 'any_laps_recorded'))

    def stage(self, args=None):
        return self.call('race', 'stage', options=args or {})

    def stop(self):
        return self.call('race', 'stop')

    def save(self):
        return self.call('race', 'save')

    def clear(self):
        return self.call('race', 'clear')


class Fields(Rpc):
    def register_race_attribute(self, field):
        return self.call('fields', 'register_race_attribute',
                         field=field_payload(field))

    def register_pilot_attribute(self, field):
        return self.call('fields', 'register_pilot_attribute',
                         field=field_payload(field))

    def register_option(self, field, panel):
        return self.call('fields', 'register_option',
                         field=field_payload(field), panel=panel)


class Config(Rpc):
    '''rhapi.config.get/set -- the server stores these as db options.'''

    def get(self, section, key, default=None):
        try:
            value = self.call('db', 'get_option', section=section, key=key)
        except Exception:
            return default
        return default if value is None else value

    def set(self, section, key, value):
        return self.call('db', 'set_option', section=section, key=key, value=value)


class Utils:
    def format_time_to_str(self, ms, leading=2, ms_sep=False):
        try:
            total = float(ms) / 1000.0
        except (TypeError, ValueError):
            return ''
        minutes = int(total // 60)
        seconds = total - minutes * 60
        if minutes:
            return '%d:%05.2f' % (minutes, seconds)
        return '%.2f' % seconds


class ServerInfo(dict):
    def __init__(self):
        super().__init__(rhapi_version=(1, 7),
                         version='4.6.0-dev.3-rust', api_version=1)


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

    def register_panel(self, domain, name, location, order=0):
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


class Events:
    '''Event registration: plugins register hooks by event name; server
    broadcasts are dispatched to them here.'''

    def __init__(self, host):
        self._host = host
        self._handlers = {}

    def on(self, event, handler):
        self._handlers.setdefault(event, []).append(handler)
        return handler

    def dispatch(self, event, args):
        for handler in self._handlers.get(event, []):
            try:
                handler(args)
            except Exception:
                print('[bridge] handler error for {}:'.format(event), file=sys.stderr)
                import traceback
                traceback.print_exc()


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
        self.server_info = ServerInfo()
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

        self.sio.on('rhapi_response', self._on_response)
        self.sio.on('rhapi_event', self._on_plugin_event)
        self.sio.on('*', self._on_any)

        self.rhapi = Rhapi(self)
        self._plugins = []

        self.sio.connect(server_url, wait_timeout=15)
        print('[bridge] connected to', server_url)

    # ---- server -> bridge ----
    def _on_response(self, data):
        with self._cond:
            call_id = (data or {}).get('id')
            if call_id is not None:
                self._acks[call_id] = data or {}
                self._cond.notify_all()

    def _on_plugin_event(self, data):
        data = data or {}
        self.rhapi.events.dispatch(data.get('event', ''), data.get('args') or {})

    def _on_any(self, event, *args):
        args_payload = args[0] if args else {}
        self.rhapi.events.dispatch(event, args_payload)

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

    # ---- plugin loading ----
    def load_plugins(self, plugins_dir, only=None):
        if not os.path.isdir(plugins_dir):
            print('[bridge] plugins dir not found:', plugins_dir)
            return
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
            spec = importlib.util.spec_from_file_location(
                'rhplugin_' + name, init_py)
            module = importlib.util.module_from_spec(spec)
            sys.modules['rhplugin_' + name] = module
            try:
                spec.loader.exec_module(module)
                if hasattr(module, 'initialize'):
                    plugin = module.initialize(self.rhapi)
                    if plugin is not None and hasattr(plugin, 'initialize'):
                        plugin.initialize(None)
                self._plugins.append(name)
                print('[bridge] loaded plugin: {} ({})'.format(
                    name, manifest.get('name', name)))
            except Exception as ex:
                print('[bridge] failed to load plugin {}: {}'.format(name, ex),
                      file=sys.stderr)
                import traceback
                traceback.print_exc()

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

    print('[bridge] running; {} plugin(s) loaded'.format(len(bridge._plugins)))
    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        print('[bridge] shutting down')
        bridge.sio.disconnect()


if __name__ == '__main__':
    main()
