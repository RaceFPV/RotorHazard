'''
Leaderboard/results characterization tests.

Runs a deterministic race scenario (laps counts differ per seat) against a
live server and validates the leaderboard math: ordering, positions, laps,
consecutives, meta fields.  Lap TIMES are timing-dependent and only checked
structurally, so the suite validates both implementations.
'''
import os
import re
import sys
import time
import unittest

try:
    import harness
except ImportError:
    sys.path.append(os.path.dirname(os.path.realpath(__file__)))
    import harness

TIME_RE = re.compile(r'^-?\d+:\d{2}\.\d{3}$')


class ResultsTest(harness.ProtocolTestCase):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.tc = cls.__new__(cls)
        cls.heat_id = cls._run_reference_race()

    @classmethod
    def _run_reference_race(cls):
        '''3 pilots: seat 0 gets 3 crossings (2 laps), seat 1 gets 2 (1 lap),
        seat 2 gets 1 (0 laps); then stop and save.'''
        resp = cls.tc.emit_and_wait('load_data', {'load_types': ['format_data']}, 'format_data')
        prev = max((f['id'] for f in resp[0]['formats']), default=0)
        fmt = cls.tc.wait_for_new_id('add_race_format', {'source_format_id': 1},
                                   'format_data', 'id', 'formats', prev, timeout=15)
        cls.tc.emit_and_wait('alter_race_format', {
            'format_id': fmt,
            'format_name': 'ResultsFmt',
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
        for _ in range(3):
            resp = cls.tc.emit_and_wait('load_data', {'load_types': ['pilot_data']}, 'pilot_data', drain=True)
            prev = max((p['pilot_id'] for p in resp[0]['pilots']), default=0)
            pilot_ids.append(cls.tc.wait_for_new_id('add_pilot', None, 'pilot_data',
                                                  'pilot_id', 'pilots', prev))

        resp = cls.tc.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data', drain=True)
        prev = max((h['id'] for h in resp[0]['heats']), default=0)
        heat = cls.tc.wait_for_new_id('add_heat', None, 'heat_data', 'id', 'heats', prev)
        resp = cls.tc.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data')
        heat_row = next(h for h in resp[0]['heats'] if h['id'] == heat)
        for slot, pid in zip(heat_row['slots'][:3], pilot_ids):
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
        for node, count in ((0, 3), (1, 2), (2, 1)):
            for _ in range(count):
                cls.client.emit('simulate_lap', {'node': node})
                time.sleep(0.6)

        # current leaderboard mid-race is gone after stop; query before stop
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

    def test_saved_race_leaderboard_math(self):
        detail = self.client.http_get_json('/api/race/{}/1'.format(self.heat_id))
        race = detail['race']
        lb = race['leaderboard']

        self.assertEqual(set(lb.keys()),
                         {'by_race_time', 'by_fastest_lap', 'by_consecutives', 'meta'})
        meta = lb['meta']
        self.assertEqual(meta['primary_leaderboard'], 'by_race_time')
        self.assertEqual(meta['win_condition'], 1)
        self.assertEqual(meta['consecutives_count'], 3)

        rows = lb['by_race_time']
        self.assertEqual(len(rows), 3)

        # ordering: most laps first
        self.assertEqual([r['laps'] for r in rows], [2, 1, 0])
        self.assertEqual([r['position'] for r in rows], [1, 2, 3])
        self.assertEqual([r['behind'] for r in rows], [0, 1, 2])

        # structural checks per row
        for row in rows:
            self.assertIn('pilot_id', row)
            self.assertIn('callsign', row)
            self.assertEqual(row['starts'], 1)
            if row['laps'] > 0:
                self.assertGreater(row['fastest_lap_raw'], 0)
                self.assertGreater(row['total_time_raw'], 0)
                self.assertGreaterEqual(row['consecutives_base'], 1)
                # average = total_time_laps / laps
                avg_expect = row['total_time_laps_raw'] / row['laps']
                self.assertAlmostEqual(row['average_lap_raw'], avg_expect, delta=1.0)
                # consecutives cover all laps when laps <= consecutives_count
                self.assertLessEqual(row['consecutives_base'], 3)
                # formatted fields match the M:SS.mmm shape
                self.assertRegex(row['total_time'], TIME_RE)
                self.assertRegex(row['fastest_lap'], TIME_RE)
                # fastest lap <= average
                self.assertLessEqual(row['fastest_lap_raw'], row['average_lap_raw'] + 0.01)
                # sources present
                self.assertEqual(row['fastest_lap_source']['round'], 1)
                self.assertEqual(row['fastest_lap_source']['heat'], self.heat_id)
            else:
                # characterized: a 0-lap pilot with a first crossing still
                # gets a position (total_time_raw > 0)
                self.assertGreater(row['total_time_raw'], 0)
                self.assertEqual(row['consecutives_base'], 0)
                self.assertEqual(row['fastest_lap_raw'], 0)
                self.assertIsNone(row['last_lap_raw'])
                self.assertEqual(row['average_lap_raw'], 0)

        # fastest-lap board contains the same pilots, ranked by fastest lap
        fl = lb['by_fastest_lap']
        self.assertEqual(sorted(r['pilot_id'] for r in fl),
                         sorted(r['pilot_id'] for r in rows))
        laps_list = [r['fastest_lap_raw'] for r in fl if r['laps'] > 0]
        self.assertEqual(laps_list, sorted(laps_list))

        # consecutives board ranks more-laps windows first
        cons = lb['by_consecutives']
        bases = [r['consecutives_base'] for r in cons]
        self.assertEqual(bases, sorted(bases, reverse=True))

        # fastest race lap meta points at the fastest pilot
        meta_fl = meta['fastest_race_lap_data']
        self.assertIn('phonetic', meta_fl)
        self.assertIn('text', meta_fl)
        self.assertEqual(meta_fl['text'][1], fl[0]['fastest_lap'])

    def test_saved_race_nodes_have_laps(self):
        detail = self.client.http_get_json('/api/race/{}/1'.format(self.heat_id))
        nodes = detail['race']['nodes']
        self.assertEqual(len(nodes[0]['laps']), 3)
        self.assertEqual(len(nodes[1]['laps']), 2)
        self.assertEqual(len(nodes[2]['laps']), 1)
        for node in nodes:
            for lap in node['laps']:
                self.assertRegex(lap['lap_time_formatted'], TIME_RE)
                self.assertGreater(lap['lap_time'], 0)

    def test_current_leaderboard_shape(self):
        '''load_data leaderboard returns the current-race payload structure.'''
        resp = self.emit_and_wait('load_data', {'load_types': ['leaderboard']}, 'leaderboard', drain=True)
        payload = resp[0]
        self.assertIn('current', payload)
        # after the saved race the current race is empty; structure must hold
        self.assertIn('leaderboard', payload['current'])


if __name__ == '__main__':
    unittest.main()