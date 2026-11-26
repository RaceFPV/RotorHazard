"""Plays FPVTrackSide: emits the plugin's client events and checks the acks.

Exits non-zero if the connector is not fully functional against the server, so
the same script serves as a manual probe and as a CI gate.
"""
import sys
import time

import socketio

url = sys.argv[1]
sio = socketio.Client()
events = []
failures = []


@sio.on('ts_lap_data')
def lap_data(payload):
    events.append(payload)


@sio.on('ts_race_marshal')
def marshal(payload):
    events.append({'marshal': payload})


def emit(event, arg, timeout=15.0):
    box = {}
    done = []

    def cb(*args):
        box['ack'] = args[0] if args else None
        done.append(True)

    sio.emit(event, arg, callback=cb)
    deadline = time.time() + timeout
    while not done and time.time() < deadline:
        time.sleep(0.05)
    ack = box.get('ack', '<no ack>')
    if ack == '<no ack>':
        failures.append('%s: no ack from the plugin' % event)
    if isinstance(ack, dict) and ack.get('error'):
        failures.append('%s: %s' % (event, ack['error']))
    return ack


sio.connect(url, wait_timeout=15)

lean = emit('ts_get_lean_mode', None)
if not isinstance(lean, dict) or 'lean_mode' not in lean:
    failures.append('lean_mode payload missing')

info = emit('ts_server_info', None) or {}
if not isinstance(info, dict) or 'version' not in info:
    failures.append('server_info missing version')

stage = {
    'p': ['Alice', 'Bob'],
    'p_id': ['ts1', 'ts2'],
    'race_id': 77,
    'race_number': 1,
    'round_number': 1,
    'bracket': None,
}
emit('ts_race_stage', stage)

# race.stage (called by the plugin) stages with tones + a random start delay,
# so the race is only RACING after ~3.5-6.5s; cross laps after that.
time.sleep(8.0)
sio.emit('simulate_lap', {'node': 0})
time.sleep(0.5)
sio.emit('simulate_lap', {'node': 0})
time.sleep(2.0)

lap_events = [e for e in events if 'seat' in e]
if not lap_events:
    failures.append('no ts_lap_data events (lap telemetry did not reach the client)')

emit('ts_race_stop', None)
emit('ts_event_info', {'name': 'Test Event'})
emit('ts_frequency_setup', [5658, 5695, 5732, 5769, 5806, 5843, 5880, 5917])
emit('ts_color_setup', {'channel_color': ['#111111', '#222222']})
emit('ts_set_lean_mode', {'lean_mode': True})
emit('ts_get_lean_mode', None)
emit('ts_race_abort', None)

# ---- marshal path (needs the saved race from the stop above) ----
time.sleep(0.5)
waveform = emit('ts_race_marshal_waveform', {'race_id': 77, 'pilot_id': 'ts1'})
if not isinstance(waveform, dict) or 'history_values' not in waveform:
    failures.append('waveform not returned for the saved race')

update = {'race_id': 77, 'pilot_id': 'ts1', 'enter_at': 300, 'exit_at': 500,
          'laps': [{'lap_time_stamp': 2500, 'lap_time': 2.5, 'peak_rssi': 90},
                   {'lap_time_stamp': 5000, 'lap_time': 2.5, 'peak_rssi': 88}]}
emit('ts_race_marshal_update', update)
time.sleep(1.5)
emit('ts_race_reset', {'race_id': 77})

if not [e for e in events if 'marshal' in e]:
    failures.append('no ts_race_marshal echo after the marshal update')

sio.disconnect()

print('lap events    :', len(lap_events))
print('marshal events:', len([e for e in events if 'marshal' in e]))
if failures:
    print('FAIL')
    for failure in failures:
        print('  -', failure)
    sys.exit(1)
print('PASS: Connector-FPVTrackSide is functional against this server')
