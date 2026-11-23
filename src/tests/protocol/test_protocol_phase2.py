'''
Phase 2 protocol tests: race list, marshal data, pilotrace details,
lap resave, and deleted-race round trip.  Also verifies heartbeat flow.
'''
import os
import sys
import time
import unittest

try:
    import harness
except ImportError:
    sys.path.append(os.path.dirname(os.path.realpath(__file__)))
    import harness


class Phase2Base(harness.ProtocolTestCase):
    '''Runs and saves one reference race for the class.'''

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.tc = cls.__new__(cls)
        cls.heat_id = cls._run_and_save_race()

    @classmethod
    def _run_and_save_race(cls, num_pilots=2):
        resp = cls.tc.emit_and_wait('load_data', {'load_types': ['format_data']}, 'format_data')
        prev = max((f['id'] for f in resp[0]['formats']), default=0)
        fmt = cls.tc.wait_for_new_id('add_race_format', {'source_format_id': 1},
                                     'format_data', 'id', 'formats', prev, timeout=15)
        cls.tc.emit_and_wait('alter_race_format', {
            'format_id': fmt,
            'format_name': 'Phase2Fmt',
            'race_time_sec': 60,
            'lap_grace_sec': 60,
            'staging_fixed_tones': 1,
            'staging_delay_tones': 0,
            'start_delay_min_ms': 100,
            'start_delay_max_ms': 300,
            'number_laps_win': 0,
            'win_condition': 1,
            'unlimited_time': 0,
        }, 'format_data', timeout=15)
        cls.tc.emit_and_wait('set_race_format', {'race_format': fmt}, 'race_status', timeout=15)

        pilot_ids = []
        for _ in range(num_pilots):
            resp = cls.tc.emit_and_wait('load_data', {'load_types': ['pilot_data']}, 'pilot_data', drain=True)
            prev = max((p['pilot_id'] for p in resp[0]['pilots']), default=0)
            pilot_ids.append(cls.tc.wait_for_new_id('add_pilot', None, 'pilot_data',
                                                    'pilot_id', 'pilots', prev))

        resp = cls.tc.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data', drain=True)
        prev = max((h['id'] for h in resp[0]['heats']), default=0)
        heat = cls.tc.wait_for_new_id('add_heat', None, 'heat_data', 'id', 'heats', prev)
        resp = cls.tc.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data')
        heat_row = next(h for h in resp[0]['heats'] if h['id'] == heat)
        for slot, pid in zip(heat_row['slots'][:num_pilots], pilot_ids):
            cls.tc.emit_and_wait('alter_heat', {'heat': heat, 'slot_id': slot['id'], 'pilot': pid},
                                 'heat_data', drain=True)
        cls.tc.emit_and_wait('set_current_heat', {'heat': heat}, 'current_heat', drain=True)

        cls.client.drain()
        cls.observer.drain()
        cls.client.emit('stage_race')
        deadline = time.monotonic() + 45
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AssertionError('race never started')
            args = cls.tc.wait_any('race_status', timeout=remaining)
            if args[0].get('race_status') == 1:
                break

        time.sleep(0.5)
        for node, count in ((0, 2), (1, 1)):
            for _ in range(count):
                cls.client.emit('simulate_lap', {'node': node})
                time.sleep(0.6)

        cls.client.emit('stop_race')
        deadline = time.monotonic() + 20
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AssertionError('race never stopped')
            args = cls.tc.wait_any('race_status', timeout=remaining)
            if args[0].get('race_status') in (2, 0):
                break
        cls.client.emit('save_laps')
        cls.tc.wait_any('race_saved', timeout=20, drain=True)
        return heat


class RaceListAndMarshalTest(Phase2Base):
    def test_race_list_contains_pilotraces(self):
        resp = self.emit_and_wait('load_data', {'load_types': ['race_list']}, 'race_list', drain=True)
        heats = resp[0]['heats']
        heat = heats[str(self.heat_id)]
        self.assertEqual(heat['heat_id'], self.heat_id)
        rounds = heat['rounds']
        self.assertIn('1', rounds)
        prs = rounds['1']['pilotraces']
        # pilotraces exist for every active-frequency seat (count varies with
        # how many frequencies earlier tests activated; >= our 2 seats)
        self.assertGreaterEqual(len(prs), 2)
        for pr in prs:
            self.assertIn('pilotrace_id', pr)
            self.assertIsNotNone(pr['pilotrace_id'])
            self.assertIn('callsign', pr)
            self.assertIn('node_index', pr)
            self.assertIn('pilot_freq', pr)
        # our two assigned pilots sit on nodes 0 and 1
        by_node = {pr['node_index']: pr for pr in prs}
        self.assertIn(0, by_node)
        self.assertIn(1, by_node)
        self.assertIsNotNone(by_node[0]['callsign'])
        self.assertIsNotNone(by_node[1]['callsign'])

    def test_get_pilotrace_roundtrip(self):
        resp = self.emit_and_wait('load_data', {'load_types': ['race_list']}, 'race_list', drain=True)
        prs = resp[0]['heats'][str(self.heat_id)]['rounds']['1']['pilotraces']
        target = next(pr for pr in prs if pr['node_index'] == 0)

        resp = self.emit_and_wait('get_pilotrace', {'pilotrace_id': target['pilotrace_id']},
                                  'race_details', drain=True)
        details = resp[0]
        self.assertEqual(details['pilotrace_id'], target['pilotrace_id'])
        self.assertEqual(details['node_index'], 0)
        self.assertEqual(len(details['laps']), 2)
        self.assertIn('enter_at', details)
        self.assertIn('exit_at', details)

        # resave with a single edited lap
        edited_lap = {
            'lap_time_stamp': details['laps'][0]['lap_time_stamp'],
            'lap_time': 5000.0,
            'source': 2,  # manual
            'deleted': False,
        }
        self.emit_and_wait('resave_laps', {
            'heat_id': self.heat_id,
            'round_id': 1,
            'callsign': target['callsign'],
            'race_id': None,
            'pilotrace_id': target['pilotrace_id'],
            'seat': 0,
            'pilot_id': target['pilot_id'],
            'laps': [edited_lap],
            'enter_at': 123,
            'exit_at': 45,
        }, 'priority_message', timeout=15, drain=True)

        resp = self.emit_and_wait('get_pilotrace', {'pilotrace_id': target['pilotrace_id']},
                                  'race_details', drain=True)
        details = resp[0]
        self.assertEqual(len(details['laps']), 1)
        self.assertAlmostEqual(details['laps'][0]['lap_time'], 5000.0, delta=0.5)
        self.assertEqual(details['laps'][0]['source'], 2)
        self.assertEqual(details['enter_at'], 123)
        self.assertEqual(details['exit_at'], 45)

    def test_current_marshal_data(self):
        '''Stopping a race enables the marshal view for the unsaved race.'''
        # stage + 1 lap + stop
        self.tc.emit_and_wait('set_current_heat', {'heat': self.heat_id}, 'current_heat', drain=True)
        self.client.drain()
        self.observer.drain()
        self.client.emit('stage_race')
        deadline = time.monotonic() + 45
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AssertionError('race never started')
            args = self.wait_any('race_status', timeout=remaining)
            if args[0].get('race_status') == 1:
                break
        self.client.emit('simulate_lap', {'node': 0})
        time.sleep(0.6)
        self.client.emit('stop_race')
        deadline = time.monotonic() + 20
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AssertionError('race never stopped')
            args = self.wait_any('race_status', timeout=remaining)
            if args[0].get('race_status') in (2, 0):
                break

        resp = self.emit_and_wait('current_race_marshal', None, 'current_marshal_data',
                                  timeout=15, drain=True)
        payload = resp[0]
        self.assertIn('race', payload)
        self.assertIn('seats', payload)
        self.assertEqual(payload['race']['round_id'], -1)
        self.assertEqual(payload['race']['heat_id'], self.heat_id)
        seat0 = payload['seats']['0']
        self.assertEqual(len(seat0['laps']), 1)
        self.assertIn('lap_time', seat0['laps'][0])

        # clean up: discard so later tests start clean
        self.client.emit('discard_laps')

    def test_z_delete_round(self):
        # runs last (name ordering): creates its own race and deletes it.
        # Restoring requires the deleted_race row id, which is only exposed
        # on the server-rendered database page (not reachable at protocol
        # level), so restore is not asserted here.
        heat = self._run_and_save_race()
        resp = self.emit_and_wait('load_data', {'load_types': ['race_list']}, 'race_list', drain=True)
        entry = resp[0]['heats'].get(str(heat))
        self.assertIsNotNone(entry)
        race_id = entry['rounds']['1']['race_id']

        resp = self.emit_and_wait('delete_race_round', {'race_id': race_id},
                                  'race_round_deleted', timeout=15, drain=True)
        self.assertTrue(resp[0]['ok'])

        # the round is gone from the race list
        resp = self.emit_and_wait('load_data', {'load_types': ['race_list']}, 'race_list', drain=True)
        entry = resp[0]['heats'].get(str(heat))
        self.assertTrue(entry is None or '1' not in entry.get('rounds', {}))


class HeartbeatTest(harness.ProtocolTestCase):
    def test_heartbeat_stream(self):
        # server broadcasts heartbeat events roughly every 500ms
        self.client.drain()
        self.observer.drain()
        time.sleep(1.3)
        count = 0
        for cl in (self.client, self.observer):
            with cl._cond:
                count += sum(1 for name, _ in cl._events if name == 'heartbeat')
        self.assertGreaterEqual(count, 2, 'expected heartbeats on connected clients')


if __name__ == '__main__':
    unittest.main()