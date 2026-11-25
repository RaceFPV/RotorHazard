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
        [--plugins-dir ./plugins] [--plugins name1,name2]

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

import socketio

RPC_OBJECT = 'rhapi_call'
RPC_TIMEOUT = 10.0


class RhapiNode:
    '''Attribute-walking proxy: rhapi.db.pilots() performs an RPC call
    ("db", "pilots") on the server.'''
    def __init__(self, host, path):
        self._host = host
        self._path = path

    def __getattr__(self, name):
        if name.startswith('_'):
            raise AttributeError(name)
        return RhapiNode(self._host, self._path + [name])

    def __call__(self, *args, **kwargs):
        method = self._path[-1] if self._path else ''
        obj = '.'.join(self._path[:-1])
        payload = {'object': obj, 'method': method, 'args': kwargs or {}}
        result = self._host.call(RPC_OBJECT, payload)
        # result is the server's full ack payload: {result: ...} or {error}
        if isinstance(result, dict) and 'error' in result:
            raise RuntimeError(result['error'])
        return result.get('result') if isinstance(result, dict) else result


class RhapiEvents:
    '''Event registration: plugins register hooks by event name; server
    broadcasts are dispatched to them here.'''
    def __init__(self, host):
        self._host = host
        self._handlers = {}

    def on(self, event, handler):
        self._handlers.setdefault(event, []).append(handler)

    def dispatch(self, event, args):
        for handler in self._handlers.get(event, []):
            try:
                handler(args)
            except Exception:
                print('[bridge] handler error for {}:'.format(event), file=sys.stderr)
                import traceback
                traceback.print_exc()


class RhapiUI:
    def __init__(self, host):
        self._host = host

    def message_speak(self, message):
        self._host.call(RPC_OBJECT, {'object': 'ui', 'method': 'message_speak',
                                     'args': {'message': message}})

    def message_notify(self, message):
        self._host.call(RPC_OBJECT, {'object': 'ui', 'method': 'message_notify',
                                     'args': {'message': message}})

    def message_alert(self, message):
        self._host.call(RPC_OBJECT, {'object': 'ui', 'method': 'message_alert',
                                     'args': {'message': message}})

    def clear_messages(self):
        self._host.call(RPC_OBJECT, {'object': 'ui', 'method': 'clear_messages', 'args': {}})

    def register_config_section(self, section, items=None):
        '''Allows a config section to be fetched via a load_data request
        (RHUI.register_config_section); items None allows the whole section.'''
        return self._host.call(RPC_OBJECT, {'object': 'ui', 'method': 'register_config_section',
                                            'args': {'section': section, 'items': items}})


class Rhapi:
    '''The object surface handed to plugins (RHAPI subset).'''
    def __init__(self, host):
        self.db = RhapiNode(host, ['db'])
        self.race = RhapiNode(host, ['race'])
        self.pilot = RhapiNode(host, ['pilot'])
        self.ui = RhapiUI(host)
        self.events = RhapiEvents(host)
        self._host = host


class Bridge:
    def __init__(self, server_url):
        self.sio = socketio.Client()
        self._call_lock = threading.Lock()
        self._call_id = 0
        self._acks = {}
        self._cond = threading.Condition()

        self.sio.on('rhapi_response', self._on_response)
        self.sio.on('rhapi_event', self._on_plugin_event)
        self.sio.on('*', self._on_any)

        self.rhapi = Rhapi(self)
        self._plugins = []

        self.sio.connect(server_url, wait_timeout=15)
        print('[bridge] connected to', server_url)

    def _on_response(self, data):
        '''Fallback response channel (server-initiated responses); the
        ack callback is the primary path.'''
        with self._cond:
            call_id = (data or {}).get('id')
            if call_id is not None:
                self._acks[call_id] = data or {}
                self._cond.notify_all()

    def _on_plugin_event(self, data):
        '''Internal-event channel: {event, args} — dispatched to plugin
        hooks by event name (the socket.io counterpart of the server's
        internal Events.trigger).'''
        data = data or {}
        self.rhapi.events.dispatch(data.get('event', ''), data.get('args') or {})

    def _on_any(self, event, *args):
        args_payload = args[0] if args else {}
        self.rhapi.events.dispatch(event, args_payload)

    def call(self, event, payload):
        '''Synchronous RPC via socket.io ack.'''
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
                    raise RuntimeError('RHAPI RPC timeout: {}'.format(payload.get('method')))
                self._cond.wait(remaining)
            return self._acks.pop(call_id)

    def _ack_callback(self, call_id):
        def cb(*args):
            with self._cond:
                # store the full server payload; __call__ unwraps 'result'
                self._acks[call_id] = args[0] if args else {}
                self._cond.notify_all()
        return cb

    def load_plugins(self, plugins_dir, only=None):
        '''Loads plugins: <dir>/<name>/__init__.py with initialize(rhapi).'''
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
                    module.initialize(self.rhapi)
                self._plugins.append(name)
                print('[bridge] loaded plugin: {} ({})'.format(
                    name, manifest.get('name', name)))
            except Exception as ex:
                print('[bridge] failed to load plugin {}: {}'.format(name, ex),
                      file=sys.stderr)

    def run_forever(self):
        try:
            while True:
                time.sleep(1)
        except KeyboardInterrupt:
            print('[bridge] shutting down')
            self.sio.disconnect()


def main():
    parser = argparse.ArgumentParser(description='RotorHazard plugin bridge')
    parser.add_argument('--server', default='http://127.0.0.1:5000')
    parser.add_argument('--plugins-dir', default='./plugins')
    parser.add_argument('--plugins', default=None,
                        help='comma-separated plugin names (default: all)')
    args = parser.parse_args()

    bridge = Bridge(args.server)
    only = set(args.plugins.split(',')) if args.plugins else None
    bridge.load_plugins(args.plugins_dir, only)
    print('[bridge] running; {} plugin(s) loaded'.format(len(bridge._plugins)))
    bridge.run_forever()


if __name__ == '__main__':
    main()
