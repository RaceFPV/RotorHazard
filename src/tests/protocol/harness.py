'''
Language-neutral protocol test harness for RotorHazard.

Speaks HTTP + Socket.IO to a running server -- either one spawned here from
the Python implementation, or an already-running server given via the
RH_TEST_URL environment variable.  The same tests therefore validate the
Python baseline now and the Rust rewrite later.

Environment variables:
  RH_TEST_URL   connect to an already-running server (e.g. http://127.0.0.1:5000)
  RH_KEEP_SERVER=1   leave the spawned server running after tests (for debugging)

Run with:  python -m unittest discover -s src/tests/protocol
'''
import atexit
import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import urllib.request

import socketio

_TESTS_DIR = os.path.dirname(os.path.realpath(__file__))
_SRC_DIR = os.path.dirname(os.path.dirname(_TESTS_DIR))  # src/
_REPO_ROOT = os.path.dirname(_SRC_DIR)

_START_TIMEOUT = 90.0


def _free_port():
    # avoid OS-ephemeral port ranges; WSL2 mirrored networking can refuse
    # connections on ports inside Windows' excluded-port reservations
    for _ in range(50):
        port = 21000 + (os.getpid() * 7 + int(time.time()) % 1000) % 8000
        try:
            with socket.socket() as sock:
                sock.bind(('127.0.0.1', port))
                return port
        except OSError:
            time.sleep(0.1)
    raise RuntimeError('Unable to find a free port in range 21000-28999')


class ServerHandle:
    '''Handle to a RotorHazard server under test.'''

    def __init__(self, base_url, proc=None, data_dir=None):
        self.base_url = base_url
        self.proc = proc
        self.data_dir = data_dir

    def stop(self):
        if self.proc:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        if self.data_dir and os.environ.get('RH_KEEP_SERVER') != '1':
            import shutil
            shutil.rmtree(self.data_dir, ignore_errors=True)


_ACTIVE_SERVER = None


def get_server():
    '''Returns (creating on first use) the server handle for this test run.'''
    global _ACTIVE_SERVER
    if _ACTIVE_SERVER is None:
        url = os.environ.get('RH_TEST_URL')
        if url:
            _ACTIVE_SERVER = ServerHandle(url.rstrip('/'))
        else:
            _ACTIVE_SERVER = _spawn_server()
            atexit.register(_shutdown_active_server)
    return _ACTIVE_SERVER


def _shutdown_active_server():
    global _ACTIVE_SERVER
    if _ACTIVE_SERVER:
        _ACTIVE_SERVER.stop()
        _ACTIVE_SERVER = None


def _spawn_server(config_overrides=None, write_config=True):
    port = _free_port()
    data_dir = tempfile.mkdtemp(prefix='rh_proto_')
    # minimal config: empty admin credentials disable socket auth (same as
    # the in-process unit tests); unspecified values fall back to defaults
    if write_config:
        config = {
            'SECRETS': {
                'ADMIN_USERNAME': '',
                'ADMIN_PASSWORD': '',
            },
            'GENERAL': {
                'HTTP_PORT': port,
                'DEBUG': False,
                # idle mocks: deterministic laps for the characterization tests
                'MOCK_NODE_SIGNAL': 0,
            },
        }
        if config_overrides:
            for section, items in config_overrides.items():
                config.setdefault(section, {}).update(items)
        with open(os.path.join(data_dir, 'config.json'), 'w') as handle:
            json.dump(config, handle)

    env = dict(os.environ)
    env['RH_DATA_DIR'] = data_dir
    env['RH_NODES'] = '8'  # run against mock nodes, no hardware required

    log_path = os.path.join(data_dir, 'server_test.log')
    log_file = open(log_path, 'w')
    proc = subprocess.Popen(
        [sys.executable, os.path.join(_SRC_DIR, 'server', 'server.py')],
        env=env, cwd=_REPO_ROOT, stdout=log_file, stderr=subprocess.STDOUT)

    server = ServerHandle('http://127.0.0.1:{}'.format(port), proc, data_dir)

    deadline = time.monotonic() + _START_TIMEOUT
    last_error = None
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(
                'Server process exited (code {}) -- log:\n{}'.format(
                    proc.returncode, _read_log_tail(log_path)))
        try:
            with urllib.request.urlopen(server.base_url + '/api/status', timeout=2) as resp:
                if 200 <= resp.status < 300:  # RH /api/status answers 201
                    return server
        except Exception as ex:  #pylint: disable=broad-except
            last_error = ex
        time.sleep(0.5)
    raise RuntimeError(
        'Server did not become ready within {}s (last error: {}) -- log:\n{}'.format(
            _START_TIMEOUT, last_error, _read_log_tail(log_path)))


def spawn_custom_server(config_overrides=None, write_config=True):
    '''Spawns an independent server instance (separate from the shared one
    used by ProtocolTestCase) for tests needing a specific configuration.
    With write_config=False the data dir starts WITHOUT a config.json so the
    server exercises its own first-run default-configuration creation.'''
    return _spawn_server(config_overrides=config_overrides, write_config=write_config)


def _read_log_tail(log_path, max_bytes=6000):
    try:
        with open(log_path, 'rb') as handle:
            handle.seek(0, 2)
            size = handle.tell()
            handle.seek(max(0, size - max_bytes))
            return handle.read().decode('utf-8', errors='replace')
    except OSError:
        return '(log unavailable)'


def spawn_rust_server(config_overrides=None, write_config=True, binary=None):
    """Spawns the Rust server binary (rust/target/debug/rh-server by
    default, overridable via RH_RUST_BIN) with the same first-run
    environment the Python spawner uses."""
    binary = binary or os.environ.get(
        'RH_RUST_BIN',
        os.path.join(_REPO_ROOT, 'rust', 'target', 'debug', 'rh-server'))
    port = _free_port()
    data_dir = tempfile.mkdtemp(prefix='rh_rust_')
    if write_config:
        config = {
            'SECRETS': {'ADMIN_USERNAME': '', 'ADMIN_PASSWORD': ''},
            'GENERAL': {'HTTP_PORT': port, 'DEBUG': False,
                        'MOCK_NODE_SIGNAL': 0},
        }
        if config_overrides:
            for section, items in config_overrides.items():
                config.setdefault(section, {}).update(items)
        with open(os.path.join(data_dir, 'config.json'), 'w') as handle:
            json.dump(config, handle)

    env = dict(os.environ)
    env['RH_DATA_DIR'] = data_dir
    env['RH_NODES'] = '8'
    env['RH_PORT'] = str(port)
    log_path = os.path.join(data_dir, 'server_test.log')
    log_file = open(log_path, 'w')
    proc = subprocess.Popen(
        [binary], env=env, cwd=_REPO_ROOT,
        stdout=log_file, stderr=subprocess.STDOUT)

    server = ServerHandle('http://127.0.0.1:{}'.format(port), proc, data_dir)
    deadline = time.monotonic() + _START_TIMEOUT
    last_error = None
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError('Rust server exited (code {}) -- log:\n{}'.format(
                proc.returncode, _read_log_tail(log_path)))
        try:
            with urllib.request.urlopen(server.base_url + '/api/status', timeout=2) as resp:
                if 200 <= resp.status < 300:
                    return server
        except Exception as ex:  #pylint: disable=broad-except
            last_error = ex
        time.sleep(0.2)
    raise RuntimeError('Rust server not ready in {}s -- log:\n{}'.format(
        _START_TIMEOUT, _read_log_tail(log_path)))


class Client:
    '''Socket.IO client wrapper with event collection and wait helpers.'''

    def __init__(self, base_url, http_session=None):
        self.base_url = base_url
        self._sio = socketio.Client(http_session=http_session)
        self._sio.on('*', self._on_any_event)
        self._cond = threading.Condition()
        self._events = []

    def _on_any_event(self, event, *args):
        with self._cond:
            self._events.append((event, args))
            self._cond.notify_all()

    def connect(self):
        self._sio.connect(self.base_url, wait_timeout=15)

    def disconnect(self):
        try:
            self._sio.disconnect()
        except Exception:  #pylint: disable=broad-except
            pass

    def emit(self, event, data=None):
        if data is None:
            self._sio.emit(event)  # some handlers take no payload at all
        else:
            self._sio.emit(event, data)

    def drain(self):
        '''Discards all events collected so far.'''
        with self._cond:
            self._events.clear()

    def wait_for(self, event, timeout=10.0, drain=False):
        '''Waits for the next event with the given name; returns its args list,
        or raises AssertionError on timeout.  With drain=True only events
        emitted after this call is made are considered.'''
        if drain:
            self.drain()
        deadline = time.monotonic() + timeout
        with self._cond:
            while True:
                for idx, (evt_name, args) in enumerate(self._events):
                    if evt_name == event:
                        del self._events[:idx + 1]
                        return args
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise AssertionError(
                        'Timed out ({:.0f}s) waiting for event "{}"; received: {}'.format(
                            timeout, event, [name for name, _ in self._events]))
                self._cond.wait(remaining)

    def emit_and_wait(self, event, data, resp_event, timeout=10.0, drain=False):
        if drain:
            self.drain()
        self.emit(event, data)
        return self.wait_for(resp_event, timeout=timeout)

    def http_get_json(self, path, timeout=10):
        with urllib.request.urlopen(self.base_url + path, timeout=timeout) as resp:
            return json.loads(resp.read().decode('utf-8'))


class ProtocolTestCase(unittest.TestCase):
    '''Base class: one shared server for the whole run, one client per test.

    Two clients are connected: 'client' sends events; 'observer' only listens.
    Responses are waited for on whichever client receives them, because some
    handlers emit with noself=True (skipping the originating socket) and
    others emit only to the requesting socket.'''

    @classmethod
    def setUpClass(cls):
        cls.server = get_server()
        cls.client = Client(cls.server.base_url)
        cls.observer = Client(cls.server.base_url)
        cls.client.connect()
        cls.observer.connect()

    @classmethod
    def tearDownClass(cls):
        cls.client.disconnect()
        cls.observer.disconnect()

    def setUp(self):
        self.client.drain()
        self.observer.drain()

    def wait_any(self, event, timeout=10.0, drain=False):
        '''Waits for the given event on either client; returns its args list.'''
        if drain:
            self.client.drain()
            self.observer.drain()
        deadline = time.monotonic() + timeout
        while True:
            for cl in (self.client, self.observer):
                with cl._cond:
                    for idx, (evt_name, args) in enumerate(cl._events):
                        if evt_name == event:
                            del cl._events[:idx + 1]
                            return args
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                received = [name for cl in (self.client, self.observer) for name, _ in cl._events]
                raise AssertionError(
                    'Timed out ({:.0f}s) waiting for event "{}"; received: {}'.format(
                        timeout, event, received[-40:]))
            for cl in (self.client, self.observer):
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                try:
                    return cl.wait_for(event, timeout=min(0.4, remaining))
                except AssertionError:
                    continue

    def wait_for_new_id(self, emit_event, emit_data, resp_event, id_field, collection_field,
                        prev_max, timeout=20.0):
        '''Emits a creation event and returns the id of the entity newer than
        prev_max, skipping stale in-flight broadcasts.'''
        self.client.drain()
        self.observer.drain()
        self.client.emit(emit_event, emit_data)
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AssertionError('Timed out waiting for new id via {}'.format(resp_event))
            args = self.wait_any(resp_event, timeout=remaining)
            ids = [item[id_field] for item in args[0][collection_field]]
            if ids and max(ids) > prev_max:
                return max(ids)

    def emit_and_wait(self, event, data, resp_event, timeout=10.0, drain=True):
        '''Emits on the primary client and waits for resp_event on either.

        Drains both queues before emitting by default, so the response is
        always a fresh event (not a stale broadcast from a previous action).'''
        if drain:
            # drain, then pause to let any in-flight broadcasts from previous
            # actions land, then drain again so the matched response is fresh
            self.client.drain()
            self.observer.drain()
            time.sleep(0.5)
            self.client.drain()
            self.observer.drain()
        self.client.emit(event, data)
        return self.wait_any(resp_event, timeout=timeout)