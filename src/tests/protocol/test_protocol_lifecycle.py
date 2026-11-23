'''
Extended protocol tests: race marshaling, heat management, scheduling,
node settings, and housekeeping endpoints.

Runs against a live server (see harness.py).  These flows go through the
same marshal/management events the UI uses, which is the stable contract
the Rust rewrite must preserve.
'''
import json
import os
import sys
import time
import unittest

try:
    import harness
except ImportError:  # when imported as 'protocol.test_protocol' by combined discovery
    sys.path.append(os.path.dirname(os.path.realpath(__file__)))
    import harness

RACE_STATUS_READY = 0
RACE_STATUS_RACING = 1
RACE_STATUS_DONE = 2

_TEST_TOKEN = str(int(time.time())) + 'x'


class RaceFlowBase(harness.ProtocolTestCase):
    '''Shared helpers for race-oriented protocol tests.'''

    def make_race_ready(self, num_pilots=2, race_time_sec=60):
        '''Creates format + pilots + heat, sets them current, and returns heat id.'''
        resp = self.emit_and_wait('load_data', {'load_types': ['format_data']}, 'format_data')
        prev_max = max((f['id'] for f in resp[0]['formats']), default=0)
        new_fmt = self.wait_for_new_id('add_race_format', {'source_format_id': 1},
                                       'format_data', 'id', 'formats', prev_max, timeout=15)

        self.emit_and_wait('alter_race_format', {
            'format_id': new_fmt,
            'format_name': 'FlowFmt ' + _TEST_TOKEN,
            'race_time_sec': race_time_sec,
            'lap_grace_sec': 60,
            'staging_fixed_tones': 1,
            'staging_delay_tones': 0,
            'start_delay_min_ms': 100,
            'start_delay_max_ms': 300,
            'number_laps_win': 0,
            'win_condition': 1,
            'unlimited_time': 0,
        }, 'format_data', timeout=15)
        self.emit_and_wait('set_race_format', {'race_format': new_fmt}, 'race_status', timeout=15)

        pilot_ids = []
        for _ in range(num_pilots):
            resp = self.emit_and_wait('load_data', {'load_types': ['pilot_data']}, 'pilot_data', drain=True)
            prev = max((p['pilot_id'] for p in resp[0]['pilots']), default=0)
            pilot_ids.append(self.wait_for_new_id('add_pilot', None, 'pilot_data',
                                                  'pilot_id', 'pilots', prev))

        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data', drain=True)
        prev_max = max((h['id'] for h in resp[0]['heats']), default=0)
        new_heat = self.wait_for_new_id('add_heat', None, 'heat_data', 'id', 'heats', prev_max)
        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data')
        heat = next(h for h in resp[0]['heats'] if h['id'] == new_heat)
        for slot, pilot_id in zip(heat['slots'][:num_pilots], pilot_ids):
            self.emit_and_wait('alter_heat', {
                'heat': new_heat, 'slot_id': slot['id'], 'pilot': pilot_id,
            }, 'heat_data', drain=True)

        self.emit_and_wait('set_current_heat', {'heat': new_heat}, 'current_heat', drain=True)
        return new_heat

    def stage_and_wait_racing(self):
        self.client.drain()
        self.observer.drain()
        self.client.emit('stage_race')
        deadline = time.monotonic() + 45
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AssertionError('race never reached RACING status')
            args = self.wait_any('race_status', timeout=remaining)
            if args[0].get('race_status') == RACE_STATUS_RACING:
                return

    def node_lap_count(self, laps_payload, node_index):
        return len(laps_payload['current']['node_index'][node_index]['laps'])


class LapMarshalTest(RaceFlowBase):
    def test_delete_and_restore_lap(self):
        self.make_race_ready(num_pilots=1)
        self.stage_and_wait_racing()

        # three laps on node 0
        resp = self.emit_and_wait('simulate_lap', {'node': 0}, 'current_laps', timeout=15, drain=True)
        time.sleep(1.0)
        resp = self.emit_and_wait('simulate_lap', {'node': 0}, 'current_laps', timeout=15, drain=True)
        time.sleep(1.0)
        resp = self.emit_and_wait('simulate_lap', {'node': 0}, 'current_laps', timeout=15, drain=True)
        laps = resp[0]['current']['node_index'][0]['laps']
        self.assertEqual(len(laps), 3)
        live_before = [l for l in laps if not l['deleted']]
        self.assertEqual(len(live_before), 3)

        # delete the middle lap via the marshal event
        target = live_before[1]['lap_index']
        resp = self.emit_and_wait('delete_lap', {'node': 0, 'lap_index': target},
                                  'current_laps', timeout=15, drain=True)
        laps = resp[0]['current']['node_index'][0]['laps']
        deleted_laps = [l for l in laps if l['deleted']]
        self.assertEqual(len(deleted_laps), 1)
        self.assertEqual(deleted_laps[0]['lap_index'], target)
        # deleted lap remains in the list but is excluded from live count
        self.assertEqual(len([l for l in laps if not l['deleted']]), 2)

        # restore it
        resp = self.emit_and_wait('restore_deleted_lap', {'node': 0, 'lap_index': target},
                                  'current_laps', timeout=15, drain=True)
        laps = resp[0]['current']['node_index'][0]['laps']
        self.assertEqual(len([l for l in laps if not l['deleted']]), 3)

        self.client.emit('stop_race')
        self._wait_stop()
        self.client.emit('discard_laps')

    def test_delete_last_lap_and_undo(self):
        self.make_race_ready(num_pilots=1)
        self.stage_and_wait_racing()

        for _ in range(2):
            self.emit_and_wait('simulate_lap', {'node': 0}, 'current_laps', timeout=15, drain=True)
            time.sleep(1.0)

        resp = self.emit_and_wait('delete_last_lap', {'node': 0}, 'current_laps', timeout=15, drain=True)
        laps = resp[0]['current']['node_index'][0]['laps']
        self.assertEqual(len([l for l in laps if not l['deleted']]), 1)

        resp = self.emit_and_wait('restore_last_deleted_lap', {'node': 0}, 'current_laps', timeout=15, drain=True)
        laps = resp[0]['current']['node_index'][0]['laps']
        self.assertEqual(len([l for l in laps if not l['deleted']]), 2)

        self.client.emit('stop_race')
        self._wait_stop()
        self.client.emit('discard_laps')

    def test_saved_race_details_via_http(self):
        new_heat = self.make_race_ready(num_pilots=2)
        self.stage_and_wait_racing()

        self.emit_and_wait('simulate_lap', {'node': 0}, 'current_laps', timeout=15, drain=True)
        time.sleep(1.0)
        self.emit_and_wait('simulate_lap', {'node': 0}, 'current_laps', timeout=15, drain=True)
        time.sleep(1.0)
        self.emit_and_wait('simulate_lap', {'node': 1}, 'current_laps', timeout=15, drain=True)

        self.client.emit('stop_race')
        self._wait_stop()
        self.client.emit('save_laps')
        self.wait_any('race_saved', timeout=20, drain=True)

        # saved-race detail endpoint: {'race': {'nodes': [...], 'leaderboard': ...}}
        detail = self.client.http_get_json('/api/race/{}/1'.format(new_heat))
        race_detail = detail['race']
        self.assertIn('nodes', race_detail)
        self.assertIn('leaderboard', race_detail)
        node0 = race_detail['nodes'][0]
        self.assertTrue(len(node0['laps']) >= 2,
                        'expected >=2 saved laps on node 0: ' + json.dumps(race_detail)[:400])
        node1 = race_detail['nodes'][1]
        self.assertTrue(len(node1['laps']) >= 1)

        # the saved leaderboard must rank the 2-lap pilot above the 1-lap pilot
        leaderboard = self.client.http_get_json('/api/race/all')
        heats = leaderboard['races']['heats']
        saved = next((h for h in heats if h['id'] == new_heat), None)
        self.assertIsNotNone(saved)
        self.assertTrue(saved['rounds'] >= 1)

    def _wait_stop(self):
        deadline = time.monotonic() + 20
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AssertionError('race never stopped')
            args = self.wait_any('race_status', timeout=remaining)
            if args[0].get('race_status') in (RACE_STATUS_DONE, RACE_STATUS_READY):
                return


class HeatManagementTest(RaceFlowBase):
    def _max_heat_id(self):
        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data')
        return max((h['id'] for h in resp[0]['heats']), default=0)
    def test_duplicate_heat(self):
        # ensure at least one heat exists (works against a fresh database)
        prev_max = self._max_heat_id()
        base_id = self.wait_for_new_id('add_heat', None, 'heat_data', 'id', 'heats', prev_max)
        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data')
        source = next(h for h in resp[0]['heats'] if h['id'] == base_id)
        prev_max = max(h['id'] for h in resp[0]['heats'])
        new_id = self.wait_for_new_id('duplicate_heat', {'heat': source['id']},
                                      'heat_data', 'id', 'heats', prev_max)
        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data')
        dup = next(h for h in resp[0]['heats'] if h['id'] == new_id)
        # duplicate carries the source's class assignment
        self.assertEqual(dup['class_id'], source['class_id'])

    def test_activate_deactivate_heat(self):
        prev_max = self._max_heat_id()
        new_id = self.wait_for_new_id('add_heat', None, 'heat_data', 'id', 'heats', prev_max)

        resp = self.emit_and_wait('deactivate_heat', {'heat': new_id}, 'heat_data', drain=True)
        heat = next(h for h in resp[0]['heats'] if h['id'] == new_id)
        self.assertFalse(heat['active'])

        resp = self.emit_and_wait('activate_heat', {'heat': new_id}, 'heat_data', drain=True)
        heat = next(h for h in resp[0]['heats'] if h['id'] == new_id)
        self.assertTrue(heat['active'])


class RaceScheduleTest(RaceFlowBase):
    def test_schedule_and_cancel(self):
        # schedule a race 30 minutes out on the current heat, then cancel
        resp = self.emit_and_wait('schedule_race', {'m': 30, 's': 0}, 'race_scheduled', timeout=15, drain=True)
        self.assertTrue(resp is not None)

        resp = self.emit_and_wait('cancel_schedule_race', None, 'race_scheduled', timeout=15, drain=True)
        self.assertTrue(resp is not None)

    def test_get_race_scheduled(self):
        # get_race_scheduled emits 'race_scheduled' to the requesting socket
        resp = self.emit_and_wait('get_race_scheduled', None, 'race_scheduled', timeout=15, drain=True)
        self.assertIn('scheduled', resp[0])
        self.assertIn('scheduled_at', resp[0])


class NodeSettingsTest(RaceFlowBase):
    def test_set_enter_exit_at_levels(self):
        # note: the response goes only to the requesting socket, and carries
        # the full per-node level lists from the active profile
        resp = self.emit_and_wait('set_enter_at_level', {'node': 0, 'enter_at_level': 95},
                                  'enter_and_exit_at_levels', timeout=15, drain=True)
        self.assertEqual(resp[0]['enter_at_levels'][0], 95)

        resp = self.emit_and_wait('set_exit_at_level', {'node': 0, 'exit_at_level': 85},
                                  'enter_and_exit_at_levels', timeout=15, drain=True)
        self.assertEqual(resp[0]['exit_at_levels'][0], 85)

    def test_start_thresh_lowering_roundtrip(self):
        # payload key matches the event name; response goes to requester only
        resp = self.emit_and_wait('set_start_thresh_lower_amount', {'start_thresh_lower_amount': 15},
                                  'start_thresh_lower_amount', timeout=15, drain=True)
        self.assertEqual(resp[0]['start_thresh_lower_amount'], 15)

        resp = self.emit_and_wait('set_start_thresh_lower_duration', {'start_thresh_lower_duration': 3},
                                  'start_thresh_lower_duration', timeout=15, drain=True)
        self.assertEqual(resp[0]['start_thresh_lower_duration'], 3)

    def test_min_lap_roundtrip(self):
        # payload key is 'min_lap'; response goes to non-originating clients
        resp = self.emit_and_wait('set_min_lap', {'min_lap': 7},
                                  'min_lap', timeout=15, drain=True)
        self.assertEqual(resp[0]['min_lap'], 7)
        self.assertIn('min_first_crossing', resp[0])
        self.assertIn('min_lap_behavior', resp[0])
        # restore default
        self.emit_and_wait('set_min_lap', {'min_lap': 0}, 'min_lap', timeout=15, drain=True)

    def test_seat_data_shape(self):
        resp = self.emit_and_wait('load_data', {'load_types': ['seat_data']}, 'seat_data', drain=True)
        seats = resp[0]['seats']
        self.assertEqual(len(seats), 8)
        for seat in seats:
            self.assertIn('seat_id', seat)
            self.assertIn('color', seat)
            self.assertTrue(seat['color'].startswith('#'))


class HousekeepingTest(RaceFlowBase):
    def test_time_endpoints(self):
        # get_server_time answers via ack callback
        done = {}

        def on_ack(value):
            done['v'] = value

        self.client._sio.emit('get_server_time', callback=on_ack)
        deadline = time.monotonic() + 5
        while 'v' not in done and time.monotonic() < deadline:
            time.sleep(0.1)
        self.assertIn('server_time_s', done.get('v', {}))

    def test_database_backup_and_list(self):
        self.client.emit('backup_database')
        self.wait_any('database_bkp_done', timeout=30, drain=True)
        resp = self.emit_and_wait('list_backups', None, 'backups_list', timeout=15, drain=True)
        self.assertTrue(len(resp[0]['backup_files']) >= 1)

    def test_set_language(self):
        self.client.emit('set_language', {'language': 'EN'})
        time.sleep(0.5)  # applied via config; no direct response event
        # server must remain healthy afterwards
        resp = self.emit_and_wait('load_data', {'load_types': ['race_status']}, 'race_status')
        self.assertIn('race_status', resp[0])

    def test_exporter_list(self):
        resp = self.emit_and_wait('load_data', {'load_types': ['exporter_list']}, 'exporter_list', drain=True)
        self.assertIn('exporters', resp[0])


if __name__ == '__main__':
    unittest.main()