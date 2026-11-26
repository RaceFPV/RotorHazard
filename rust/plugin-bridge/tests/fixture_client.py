"""Drives the fixture plugin: proves the bridge surface end to end, fast."""
import sys
import time

import socketio

url = sys.argv[1]
sio = socketio.Client()
seen = {'ui': [], 'lap': []}
failures = []


@sio.on('ui')
def on_ui(msg):
    seen['ui'].append(msg)


@sio.on('ts_fixture_lap')
def on_lap(msg):
    seen['lap'].append(msg)


def emit(event, arg, timeout=20.0):
    box, done = {}, []

    def cb(*args):
        box['ack'] = args[0] if args else None
        done.append(True)

    sio.emit(event, arg, callback=cb)
    deadline = time.time() + timeout
    while not done and time.time() < deadline:
        time.sleep(0.05)
    if 'ack' not in box:
        failures.append('%s: no ack (bridge did not answer)' % event)
        return None
    ack = box['ack']
    if isinstance(ack, dict) and ack.get('error'):
        failures.append('%s: %s' % (event, ack['error']))
    return ack


sio.connect(url, wait_timeout=15)

# 1. the plugin's panel must be served with its option
sio.emit('load_data', {'load_types': [{'type': 'ui', 'value': 'settings'}]})
time.sleep(1.0)
panels = [p['panel']['name'] for page in seen['ui'] for p in page.get('panels', [])]
if 'fixture' not in panels:
    failures.append('plugin panel missing from load_data ui (got %r)' % (panels,))
else:
    print('panel          :', 'fixture')

# 2. every RHAPI call the fixture makes must succeed
probe = emit('ts_probe', None) or {}
if probe.get('failures'):
    for failure in probe['failures']:
        failures.append('RHAPI %s -> %s' % (failure['call'], failure['error']))
print('RHAPI calls    :', probe.get('ok_count'), 'ok,',
      len(probe.get('failures') or []), 'failed')

# 3. checkbox round-trip stores 1/0 like RHData.set_option
on = emit('ts_fixture_write', {'value': True}) or {}
off = emit('ts_fixture_write', {'value': False}) or {}
if on.get('stored') != '1':
    failures.append("option true stored as %r, expected '1'" % (on.get('stored'),))
if off.get('stored') != '0':
    failures.append("option false stored as %r, expected '0'" % (off.get('stored'),))
print('checkbox       :', on.get('stored'), '/', off.get('stored'))

# 4. stage through RHAPI, then let mock laps flow to the plugin
stage = emit('ts_fixture_stage', None) or {}
if not stage.get('heat'):
    failures.append('staging did not return a heat id')
else:
    print('staged heat    :', stage.get('heat'), 'pilots', stage.get('pilots'))

time.sleep(8.0)          # staging tones + start delay
sio.emit('simulate_lap', {'node': 0})
time.sleep(0.5)
sio.emit('simulate_lap', {'node': 0})
time.sleep(2.0)

print('lap events     :', len(seen['lap']))
if not seen['lap']:
    failures.append('no ts_fixture_lap events (raceLapRecorded did not reach the plugin)')

# 5. the plugin's own counter must agree
again = emit('ts_probe', None) or {}
if again.get('laps_seen', 0) < 1:
    failures.append('plugin counted no laps (laps_seen=%r)' % (again.get('laps_seen'),))

sio.disconnect()

if failures:
    print('FAIL')
    for failure in failures:
        print('  -', failure)
    sys.exit(1)
print('PASS: plugin bridge fixture')
