"""Plays FPVTrackSide: emits the plugin's client events and checks the acks."""
import sys
import time

import socketio

url = sys.argv[1]
sio = socketio.Client()
events = []


@sio.on('ts_lap_data')
def lap_data(payload):
    events.append(payload)


@sio.on('ts_race_marshal')
def marshal(payload):
    events.append({'marshal': payload})


@sio.event
def connect():
    print('connected')


def emit(event, arg, timeout=12.0):
    box = {}
    done = []

    def cb(*args):
        box['ack'] = args[0] if args else None
        done.append(True)

    sio.emit(event, arg, callback=cb)
    deadline = time.time() + timeout
    while not done and time.time() < deadline:
        time.sleep(0.05)
    return box.get('ack', '<no ack>')


sio.connect(url, wait_timeout=15)

print('lean_mode :', emit('ts_get_lean_mode', None))
print('server_info:', str(emit('ts_server_info', None))[:120])

stage = {
    'p': ['Alice', 'Bob'],
    'p_id': ['ts1', 'ts2'],
    'race_id': 77,
    'race_number': 1,
    'round_number': 1,
    'bracket': None,
}
print('stage     :', emit('ts_race_stage', stage))

# race.stage (called by the plugin) stages with tones + a random start delay,
# so the race is only RACING after ~3.5-6.5s; cross laps after that.
time.sleep(8.0)
sio.emit('simulate_lap', {'node': 0})
time.sleep(0.5)
sio.emit('simulate_lap', {'node': 0})
time.sleep(2.0)

print('lap events:', events)
print('stop      :', emit('ts_race_stop', None))
print('event_info:', emit('ts_event_info', {'name': 'Test Event'}))
print('freq_setup:', emit('ts_frequency_setup', [5658, 5695, 5732, 5769, 5806, 5843, 5880, 5917]))
print('color_setup:', emit('ts_color_setup', {'channel_color': ['#111111', '#222222']}))
print('set_lean  :', emit('ts_set_lean_mode', {'lean_mode': True}))
print('get_lean  :', emit('ts_get_lean_mode', None))
print('abort     :', emit('ts_race_abort', None))

# ---- marshal path (needs the saved race from the stop above) ----
time.sleep(0.5)
print('waveform  :', str(emit('ts_race_marshal_waveform',
                              {'race_id': 77, 'pilot_id': 'ts1'}))[:160])
update = {'race_id': 77, 'pilot_id': 'ts1', 'enter_at': 300, 'exit_at': 500,
          'laps': [{'lap_time_stamp': 2500, 'lap_time': 2.5, 'peak_rssi': 90},
                   {'lap_time_stamp': 5000, 'lap_time': 2.5, 'peak_rssi': 88}]}
print('marshal   :', emit('ts_race_marshal_update', update))
time.sleep(1.0)
print('reset     :', emit('ts_race_reset', {'race_id': 77}))
print('marshal events:', [e for e in events if 'marshal' in e][:2])
sio.disconnect()
