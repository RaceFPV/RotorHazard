'''
Performance comparison harness: runs identical scenarios against the Python
and Rust RotorHazard servers and prints a comparison table.

Scenarios:
  1. boot-to-ready (fresh data dir, timed process start -> /api/status OK)
  2. resident memory (RSS) at idle
  3. HTTP API latency: 200 GET /api/status (mean / p50 / p95 / max)
  4. socket round-trip latency: 100 get_server_time acks (mean / p50 / p95)
  5. lap-processing throughput: 300 simulate_lap events, wall time + laps/sec
  6. memory + CPU under sustained heartbeat load with 10 connected clients

Usage:
  python3 bench.py                 # runs both servers and compares
  python3 bench.py --only rust     # only one side
'''
import argparse
import json
import os
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.request

sys.path.insert(0, os.path.join(os.path.dirname(os.path.realpath(__file__)),
                                '..', '..', 'src', 'tests', 'protocol'))
import harness  # noqa: E402

REPO_ROOT = harness._REPO_ROOT
VENV_PY = os.environ.get('RH_BENCH_PY', 'python3')
RESULTS = {}


def proc_stats(pid):
    """RSS kB and cumulative CPU seconds for a pid."""
    rss = 0
    with open(f'/proc/{pid}/status') as f:
        for line in f:
            if line.startswith('VmRSS:'):
                rss = int(line.split()[1])
                break
    with open(f'/proc/{pid}/stat') as f:
        parts = f.read().split()
        cpu = (int(parts[13]) + int(parts[14])) / os.sysconf('SC_CLK_TCK')
    return rss, cpu


def spawn(which):
    """Starts a server; returns (ServerHandle, boot_ms)."""
    if which == 'rust':
        t0 = time.monotonic()
        srv = harness.spawn_rust_server({
            'GENERAL': {'MOCK_NODE_SIGNAL': 0},
        })
        boot = (time.monotonic() - t0) * 1000
        return srv, boot
    else:
        t0 = time.monotonic()
        srv = harness.get_server if False else None
        # direct spawn of the Python server on a free port
        import socket as _socket
        s = _socket.socket()
        s.bind(('127.0.0.1', 0))
        port = s.getsockname()[1]
        s.close()
        data_dir = tempfile.mkdtemp(prefix='rh_bench_py_')
        # the Python server requires the data dir to pre-exist, and reads
        # its port from config.json (it ignores RH_PORT)
        os.makedirs(data_dir, exist_ok=True)
        with open(os.path.join(data_dir, 'config.json'), 'w') as fh:
            json.dump({'SECRETS': {'ADMIN_USERNAME': '', 'ADMIN_PASSWORD': ''},
                       'GENERAL': {'HTTP_PORT': port, 'DEBUG': False,
                                    'MOCK_NODE_SIGNAL': 0}}, fh)
        env = dict(os.environ)
        env.pop('RH_PORT', None)
        env['RH_DATA_DIR'] = data_dir
        env['RH_NODES'] = '8'
        t0 = time.monotonic()
        proc = subprocess.Popen(
            [VENV_PY, os.path.join(REPO_ROOT, 'src', 'server', 'server.py')],
            env=env, cwd=REPO_ROOT,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        base = f'http://127.0.0.1:{port}'
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            try:
                with urllib.request.urlopen(base + '/api/status', timeout=2) as r:
                    if 200 <= r.status < 300:
                        break
            except Exception:
                time.sleep(0.1)
        else:
            raise RuntimeError('python server not ready')
        boot = (time.monotonic() - t0) * 1000

        class Wrap:
            def stop(self):
                self.proc.terminate()
        srv = Wrap()
        srv.base_url = base
        srv.proc = proc
        srv.data_dir = data_dir
        return srv, boot


def http_latency(base, count=200):
    """Sequential GET /api/status latency stats in ms."""
    lat = []
    for _ in range(count):
        t0 = time.monotonic()
        with urllib.request.urlopen(base + '/api/status', timeout=5) as r:
            r.read()
        lat.append((time.monotonic() - t0) * 1000)
    lat.sort()
    return {
        'mean': statistics.mean(lat),
        'p50': lat[len(lat) // 2],
        'p95': lat[int(len(lat) * 0.95)],
        'max': lat[-1],
    }


def socket_latency(client, count=100):
    """get_server_time ack round-trip latency stats in ms."""
    lat = []
    for _ in range(count):
        t0 = time.monotonic()
        done = {}
        ev = __import__('threading').Event()

        def on_ack(value):
            done['v'] = value
            ev.set()

        client._sio.emit('get_server_time', callback=on_ack)
        ev.wait(5)
        lat.append((time.monotonic() - t0) * 1000)
    lat.sort()
    return {
        'mean': statistics.mean(lat),
        'p50': lat[len(lat) // 2],
        'p95': lat[int(len(lat) * 0.95)],
        'max': lat[-1],
    }


def wait_new_id(client, resp_event, id_field, collection_field, prev_max, timeout=20.0):
    """Waits for a broadcast collection containing an id newer than prev_max."""
    deadline = time.monotonic() + timeout
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise RuntimeError('timed out waiting for new id')
        args = client.wait_for(resp_event, timeout=remaining)
        ids = [item[id_field] for item in args[0][collection_field]]
        if ids and max(ids) > prev_max:
            return max(ids)


def lap_throughput(client, count=300):
    observer = harness.Client(client.base_url)
    observer.connect()
    """Fires `count` simulate_lap events; returns (wall_s, laps_sec)."""
    # lift the minimum-lap constraint so rapid-fire laps all register
    client.emit('set_option', {'option': 'MinLapSec', 'value': 0})
    time.sleep(0.5)

    # set up a race: pilots + heat + stage
    resp = client.emit_and_wait('load_data', {'load_types': ['pilot_data']},
                                'pilot_data', drain=True)
    prev = max((p['pilot_id'] for p in resp[0]['pilots']), default=0)
    client.drain()
    client.emit('add_pilot')
    pid = wait_new_id(client, 'pilot_data', 'pilot_id', 'pilots', prev)
    resp = client.emit_and_wait('load_data', {'load_types': ['heat_data']},
                                'heat_data', drain=True)
    prev = max((h['id'] for h in resp[0]['heats']), default=0)
    client.drain()
    client.emit('add_heat')
    heat = wait_new_id(client, 'heat_data', 'id', 'heats', prev)
    resp = client.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data')
    heat_row = next(h for h in resp[0]['heats'] if h['id'] == heat)
    client.emit('alter_heat', {'heat': heat, 'slot_id': heat_row['slots'][0]['id'],
               'pilot': pid})
    observer.wait_for('heat_data', timeout=10, drain=True)
    client.emit_and_wait('set_current_heat', {'heat': heat}, 'current_heat', drain=True)

    client.drain()
    observer.drain()
    client.emit('stage_race')
    deadline = time.monotonic() + 45
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise RuntimeError('race never started')
        try:
            args = client.wait_for('race_status', timeout=min(remaining, 0.5))
        except AssertionError:
            continue
        if args[0].get('race_status') == 1:
            break

    client.drain()
    t0 = time.monotonic()
    for i in range(count):
        client.emit('simulate_lap', {'node': i % 2})
    # wait until all laps are visible
    while time.monotonic() - t0 < 300:
        try:
            args = client.wait_for('current_laps', timeout=2)
        except AssertionError:
            continue
        total = sum(len(node['laps']) for node in args[0]['current']['node_index'])
        if total >= count:
            break
    wall = time.monotonic() - t0
    client.emit('stop_race')
    time.sleep(0.5)
    client.emit('discard_laps')
    time.sleep(0.5)
    observer.disconnect()
    return wall, count / wall


def fanout_load(base, clients_n=10, seconds=10):
    """Connects clients, measures server RSS/CPU while heartbeats stream."""
    import threading
    procs = []
    # find the server pid via /proc scan on cmdline
    for pid in os.listdir('/proc'):
        if not pid.isdigit():
            continue
        try:
            with open(f'/proc/{pid}/cmdline', 'rb') as f:
                cmd = f.read().decode(errors='replace')
            if 'rust/target/' in cmd and 'rh-server' in cmd:
                procs.append(int(pid))
            elif 'src/server/server.py' in cmd and 'python' in cmd:
                procs.append(int(pid))
        except OSError:
            continue
    rss0, cpu0 = proc_stats(procs[0])
    socks = []
    for _ in range(clients_n):
        c = harness.Client(base)
        c.connect()
        socks.append(c)
    t0 = time.monotonic()
    time.sleep(seconds)
    wall = time.monotonic() - t0
    rss1, cpu1 = proc_stats(procs[0])
    for c in socks:
        c.disconnect()
    return {
        'rss_kb': rss1,
        'rss_delta_kb': rss1 - rss0,
        'cpu_percent': (cpu1 - cpu0) / wall * 100.0,
    }


def run_side(which, quick=False):
    print(f'== {which} ==')
    srv, boot = spawn(which)
    res = {'boot_ms': boot}
    time.sleep(1.0)  # settle
    rss, _ = proc_stats(srv.proc.pid)
    res['rss_idle_kb'] = rss

    res['http'] = http_latency(srv.base_url, 100 if quick else 200)

    client = harness.Client(srv.base_url)
    client.connect()
    res['socket'] = socket_latency(client, 50 if quick else 100)

    wall, rate = lap_throughput(client, 100 if quick else 300)
    res['lap_wall_s'] = wall
    res['lap_rate'] = rate

    fan = fanout_load(srv.base_url, 10, 6 if quick else 10)
    res['rss_loaded_kb'] = fan['rss_kb']
    res['cpu_percent'] = fan['cpu_percent']

    client.disconnect()
    RESULTS[which] = res
    print(json.dumps(res, indent=1))
    srv.stop()
    if hasattr(srv, 'proc') and srv.proc.poll() is None:
        srv.proc.terminate()
    return res


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--only', choices=['python', 'rust'])
    parser.add_argument('--quick', action='store_true')
    args = parser.parse_args()

    sides = [args.only] if args.only else ['python', 'rust']
    for side in sides:
        run_side(side, quick=args.quick)

    if len(RESULTS) == 2:
        py, ru = RESULTS['python'], RESULTS['rust']
        print()
        print(f'{"metric":<28}{"python":>14}{"rust":>14}{"delta":>12}')
        print('-' * 68)

        def row(name, a, b, fmt='{}'):
            try:
                delta = f'{(b / a - 1) * 100:+.0f}%'
            except Exception:
                delta = 'n/a'
            print(f'{name:<28}{fmt.format(a):>14}{fmt.format(b):>14}{delta:>12}')

        row('boot-to-ready (ms)', py['boot_ms'], ru['boot_ms'], '{:.0f}')
        row('RSS idle (kB)', py['rss_idle_kb'], ru['rss_idle_kb'], '{:.0f}')
        row('RSS loaded (kB)', py['rss_loaded_kb'], ru['rss_loaded_kb'], '{:.0f}')
        row('http mean (ms)', py['http']['mean'], ru['http']['mean'], '{:.2f}')
        row('http p95 (ms)', py['http']['p95'], ru['http']['p95'], '{:.2f}')
        row('socket mean (ms)', py['socket']['mean'], ru['socket']['mean'], '{:.2f}')
        row('socket p95 (ms)', py['socket']['p95'], ru['socket']['p95'], '{:.2f}')
        row('lap wall 300 (s)', py['lap_wall_s'], ru['lap_wall_s'], '{:.2f}')
        row('lap rate (laps/s)', py['lap_rate'], ru['lap_rate'], '{:.1f}')
        row('cpu under load (%)', py['cpu_percent'], ru['cpu_percent'], '{:.1f}')


if __name__ == '__main__':
    main()