'''
Protocol-level characterization tests for RotorHazard.

These tests exercise a running server purely over HTTP + Socket.IO (no
in-process imports), so the same suite validates the Python implementation
(baseline) and, later, the Rust rewrite.  See harness.py for how the server
under test is selected.

The server boots with mock nodes (RH_NODES=8) and an empty database; tests
create their own uniquely-named entities so they stay independent.
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
RACE_STATUS_STAGING = 3

_TEST_TOKEN = str(int(time.time()))  # makes entity names unique within a run


def wait_for_race_status(testcase, target_status, timeout=45.0, drain=False, allow_ready=False):
    '''Waits until a race_status broadcast (on either client) reports target_status.'''
    accepted = {target_status, RACE_STATUS_READY} if allow_ready else {target_status}
    deadline = time.monotonic() + timeout
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise AssertionError('Timed out waiting for race_status=={}'.format(target_status))
        args = testcase.wait_any('race_status', timeout=remaining, drain=drain)
        drain = False
        if args and args[0].get('race_status') in accepted:
            return args[0]


class DataLoadTest(harness.ProtocolTestCase):
    def test_load_data_payloads(self):
        self.client.emit('load_data', {'load_types': [
            'pilot_data', 'heat_data', 'class_data', 'format_data',
            'frequency_data', 'node_data', 'race_status']})
        pilots = self.wait_any('pilot_data')[0]
        self.assertIn('pilots', pilots)
        self.assertIsInstance(pilots['pilots'], list)

        heats = self.wait_any('heat_data')[0]
        self.assertIn('heats', heats)
        self.assertIsInstance(heats['heats'], list)

        classes = self.wait_any('class_data')[0]
        self.assertIn('classes', classes)

        formats = self.wait_any('format_data')[0]
        self.assertIn('formats', formats)
        self.assertTrue(len(formats['formats']) >= 1)
        fmt = formats['formats'][0]
        for key in ('id', 'name', 'unlimited_time', 'race_time_sec',
                    'lap_grace_sec', 'staging_fixed_tones', 'win_condition'):
            self.assertIn(key, fmt)

        freqs = self.wait_any('frequency_data')[0]
        self.assertIn('fdata', freqs)
        self.assertEqual(len(freqs['fdata']), 8)

        node_data = self.wait_any('node_data')[0]
        for key in ('node_peak_rssi', 'node_nadir_rssi', 'pass_peak_rssi',
                    'pass_nadir_rssi', 'debug_pass_count'):
            self.assertIn(key, node_data)
            self.assertEqual(len(node_data[key]), 8)

        status = self.wait_any('race_status')[0]
        self.assertIn('race_status', status)

    def test_http_api_status(self):
        status = self.client.http_get_json('/api/status')
        info = status['status']['server_info']
        self.assertIn('server_api', info)
        self.assertIn('release_version', info)
        self.assertEqual(status['status']['state']['num_nodes'], 8)


class PilotCrudTest(harness.ProtocolTestCase):
    def test_add_alter_delete_pilot(self):
        resp = self.emit_and_wait('load_data', {'load_types': ['pilot_data']}, 'pilot_data')
        count_before = len(resp[0]['pilots'])
        prev_max = max((p['pilot_id'] for p in resp[0]['pilots']), default=0)

        new_pilot = self.wait_for_new_id('add_pilot', None, 'pilot_data', 'pilot_id', 'pilots', prev_max)
        resp = self.emit_and_wait('alter_pilot', {
            'pilot_id': new_pilot,
            'callsign': 'TST ' + _TEST_TOKEN,
            'team_name': 'team T',
            'phonetic': 'testy',
            'name': 'Test Pilot',
        }, 'pilot_data')
        altered = next(p for p in resp[0]['pilots'] if p['pilot_id'] == new_pilot)
        self.assertEqual(altered['callsign'], 'TST ' + _TEST_TOKEN)
        self.assertEqual(altered['team'], 'team T')
        self.assertEqual(altered['phonetic'], 'testy')

        self.client.emit('delete_pilot', {'pilot': new_pilot})
        resp = self.wait_any('pilot_data', drain=True)[0]
        self.assertEqual(len(resp['pilots']), count_before)
        self.assertFalse(any(p['pilot_id'] == new_pilot for p in resp['pilots']))

    def test_http_api_pilot_roundtrip(self):
        resp = self.emit_and_wait('load_data', {'load_types': ['pilot_data']}, 'pilot_data')
        prev_max = max((p['pilot_id'] for p in resp[0]['pilots']), default=0)
        new_pilot = self.wait_for_new_id('add_pilot', None, 'pilot_data', 'pilot_id', 'pilots', prev_max)

        api_pilot = self.client.http_get_json('/api/pilot/{}'.format(new_pilot))
        # the HTTP API serializes the model directly (key 'id'), while the
        # socket payloads use 'pilot_id' -- this asymmetry is part of the
        # protocol contract and must be preserved by the rewrite
        self.assertEqual(api_pilot['pilot']['id'], new_pilot)

        api_all = self.client.http_get_json('/api/pilot/all')
        # note: HTTP API uses model key 'id' here as well
        self.assertTrue(any(p['id'] == new_pilot for p in api_all['pilots']))


class RaceClassCrudTest(harness.ProtocolTestCase):
    def _max_class_id(self):
        resp = self.emit_and_wait('load_data', {'load_types': ['class_data']}, 'class_data')
        return max((c['id'] for c in resp[0]['classes']), default=0)

    def test_add_alter_delete_class(self):
        prev_max = self._max_class_id()
        new_class = self.wait_for_new_id('add_race_class', {
            'name': 'TestClass ' + _TEST_TOKEN,
            'description': 'protocol test class',
        }, 'class_data', 'id', 'classes', prev_max)

        resp = self.emit_and_wait('alter_race_class', {
            'class_id': new_class,
            'class_name': 'TestClassRenamed ' + _TEST_TOKEN,
            'class_description': 'renamed',
        }, 'class_data')
        altered = next(c for c in resp[0]['classes'] if c['id'] == new_class)
        self.assertEqual(altered['name'], 'TestClassRenamed ' + _TEST_TOKEN)

        self.client.emit('delete_class', {'class': new_class})
        resp = self.wait_any('class_data', drain=True)[0]
        self.assertFalse(any(c['id'] == new_class for c in resp['classes']))

    def test_http_api_class_roundtrip(self):
        prev_max = self._max_class_id()
        new_class = self.wait_for_new_id('add_race_class', {
            'name': 'TestClassAPI ' + _TEST_TOKEN,
        }, 'class_data', 'id', 'classes', prev_max)

        api = self.client.http_get_json('/api/class/{}'.format(new_class))
        self.assertEqual(api['class']['id'], new_class)


class HeatCrudTest(harness.ProtocolTestCase):
    def _max_heat_id(self):
        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data')
        return max((h['id'] for h in resp[0]['heats']), default=0)

    def _new_pilot(self):
        resp = self.emit_and_wait('load_data', {'load_types': ['pilot_data']}, 'pilot_data')
        prev_max = max((p['pilot_id'] for p in resp[0]['pilots']), default=0)
        return self.wait_for_new_id('add_pilot', None, 'pilot_data', 'pilot_id', 'pilots', prev_max)

    def test_add_alter_heat_and_assign_pilot(self):
        prev_max = self._max_heat_id()
        new_heat = self.wait_for_new_id('add_heat', None, 'heat_data', 'id', 'heats', prev_max)
        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data')
        heat = next(h for h in resp[0]['heats'] if h['id'] == new_heat)
        self.assertTrue(len(heat['slots']) >= 8)
        slot_id = heat['slots'][0]['id']

        pilot_id = self._new_pilot()
        resp = self.emit_and_wait('alter_heat', {
            'heat': new_heat,
            'name': 'TestHeat ' + _TEST_TOKEN,
        }, 'heat_data', drain=True)
        heat = next(h for h in resp[0]['heats'] if h['id'] == new_heat)
        self.assertEqual(heat['name'], 'TestHeat ' + _TEST_TOKEN)

        resp = self.emit_and_wait('alter_heat', {
            'heat': new_heat,
            'slot_id': slot_id,
            'pilot': pilot_id,
        }, 'heat_data', drain=True)
        heat = next(h for h in resp[0]['heats'] if h['id'] == new_heat)
        self.assertEqual(heat['slots'][0]['pilot_id'], pilot_id)

    def test_set_current_heat(self):
        prev_max = self._max_heat_id()
        new_heat = self.wait_for_new_id('add_heat', None, 'heat_data', 'id', 'heats', prev_max)
        resp = self.emit_and_wait('set_current_heat', {'heat': new_heat}, 'current_heat', drain=True)
        self.assertEqual(resp[0]['current_heat'], new_heat)

    def test_delete_heat(self):
        prev_max = self._max_heat_id()
        new_heat = self.wait_for_new_id('add_heat', None, 'heat_data', 'id', 'heats', prev_max)
        self.client.emit('delete_heat', {'heat': new_heat})
        resp = self.wait_any('heat_data', drain=True)[0]
        self.assertFalse(any(h['id'] == new_heat for h in resp['heats']))


class RaceFormatTest(harness.ProtocolTestCase):
    def test_add_alter_set_format(self):
        resp = self.emit_and_wait('load_data', {'load_types': ['format_data']}, 'format_data')
        prev_max = max((f['id'] for f in resp[0]['formats']), default=0)
        new_fmt = self.wait_for_new_id('add_race_format', {'source_format_id': 1},
                                       'format_data', 'id', 'formats', prev_max)

        resp = self.emit_and_wait('alter_race_format', {
            'format_id': new_fmt,
            'format_name': 'TestFmtR ' + _TEST_TOKEN,
            'race_time_sec': 45,
            'staging_fixed_tones': 1,
        }, 'format_data', drain=True)
        altered = next(f for f in resp[0]['formats'] if f['id'] == new_fmt)
        self.assertEqual(altered['name'], 'TestFmtR ' + _TEST_TOKEN)
        self.assertEqual(altered['race_time_sec'], 45)

        resp = self.emit_and_wait('set_race_format', {'race_format': new_fmt}, 'race_status', drain=True)
        self.assertEqual(resp[0]['race_format_id'], new_fmt)

    def test_http_api_formats(self):
        api = self.client.http_get_json('/api/format/all')
        self.assertIn('formats', api)
        self.assertTrue(len(api['formats']) >= 1)


class FrequencyTest(harness.ProtocolTestCase):
    def test_set_frequency(self):
        resp = self.emit_and_wait('set_frequency', {
            'node': 0,
            'band': 'F',
            'channel': 4,
            'frequency': 5800,
        }, 'frequency_data', drain=True)
        self.assertEqual(resp[0]['fdata'][0]['frequency'], 5800)
        self.assertEqual(resp[0]['fdata'][0]['band'], 'F')

    def test_frequency_preset(self):
        resp = self.emit_and_wait('set_frequency_preset', {
            'profile': None,  # current profile
            'preset': 'RB',
        }, 'frequency_data', timeout=15, drain=True)
        # RB preset assigns factory band/channels to the first 6 nodes
        first6 = [f['frequency'] for f in resp[0]['fdata'][:6]]
        self.assertTrue(all(f > 0 for f in first6))

    def test_add_profile(self):
        resp = self.emit_and_wait('add_profile', {
            'name': 'TestProfile ' + _TEST_TOKEN,
        }, 'frequency_data', timeout=15, drain=True)
        # a new profile must not disturb the per-node frequency list shape
        self.assertEqual(len(resp[0]['fdata']), 8)


class RaceLifecycleTest(HeatCrudTest):
    '''Full race flow: format -> pilots -> heat -> stage -> laps -> save.'''

    def _setup_heat_with_pilots(self, num_pilots=2):
        resp = self.emit_and_wait('load_data', {'load_types': ['format_data']}, 'format_data')
        prev_max = max((f['id'] for f in resp[0]['formats']), default=0)
        new_fmt = self.wait_for_new_id('add_race_format', {'source_format_id': 1},
                                       'format_data', 'id', 'formats', prev_max, timeout=15)

        self.emit_and_wait('alter_race_format', {
            'format_id': new_fmt,
            'format_name': 'RaceLifecycleFmt ' + _TEST_TOKEN,
            'race_time_sec': 30,
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
            prev_max = max((p['pilot_id'] for p in resp[0]['pilots']), default=0)
            pilot_ids.append(self.wait_for_new_id('add_pilot', None, 'pilot_data',
                                                  'pilot_id', 'pilots', prev_max))

        prev_max_heat = self._max_heat_id()
        new_heat = self.wait_for_new_id('add_heat', None, 'heat_data', 'id', 'heats', prev_max_heat)
        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data')
        heat = next(h for h in resp[0]['heats'] if h['id'] == new_heat)

        for slot, pilot_id in zip(heat['slots'][:num_pilots], pilot_ids):
            self.emit_and_wait('alter_heat', {
                'heat': new_heat,
                'slot_id': slot['id'],
                'pilot': pilot_id,
            }, 'heat_data', drain=True)

        self.emit_and_wait('set_current_heat', {'heat': new_heat}, 'current_heat', drain=True)
        return new_heat

    def test_stage_race_simulate_laps_save(self):
        new_heat = self._setup_heat_with_pilots(num_pilots=2)

        self.client.emit('stage_race')
        wait_for_race_status(self, RACE_STATUS_RACING, timeout=45)

        # simulate two laps on node 0, one on node 1
        # (current_laps payload: {'current': {'node_index': [<per-node results>]}})
        resp = self.emit_and_wait('simulate_lap', {'node': 0}, 'current_laps', timeout=15, drain=True)
        node0_laps = resp[0]['current']['node_index'][0]['laps']
        self.assertTrue(len(node0_laps) >= 1,
                        'expected >=1 lap on node 0, got: ' + json.dumps(resp[0], default=str))

        time.sleep(0.6)
        resp = self.emit_and_wait('simulate_lap', {'node': 0}, 'current_laps', timeout=15, drain=True)
        node0_laps = resp[0]['current']['node_index'][0]['laps']
        self.assertTrue(len(node0_laps) >= 2)

        resp = self.emit_and_wait('simulate_lap', {'node': 1}, 'current_laps', timeout=15, drain=True)
        node1_laps = resp[0]['current']['node_index'][1]['laps']
        self.assertTrue(len(node1_laps) >= 1)

        # stop the race and save it (status may pass through DONE or go
        # straight to READY when there is nothing to save)
        self.client.emit('stop_race')
        wait_for_race_status(self, RACE_STATUS_DONE, timeout=20, allow_ready=True)
        self.client.emit('save_laps')
        self.wait_any('race_saved', timeout=20, drain=True)

        # saved race must be queryable over HTTP with rounds recorded
        races = self.client.http_get_json('/api/race/all')
        heats = races['races']['heats']
        saved = next((h for h in heats if h['id'] == new_heat), None)
        self.assertIsNotNone(saved, 'heat {} missing from /api/race/all'.format(new_heat))
        self.assertTrue(saved['rounds'] >= 1, 'no rounds recorded for heat {}'.format(new_heat))

    def test_stage_and_stop_without_save(self):
        self._setup_heat_with_pilots(num_pilots=1)
        self.client.emit('stage_race')
        wait_for_race_status(self, RACE_STATUS_RACING, timeout=45)
        self.client.emit('stop_race')
        wait_for_race_status(self, RACE_STATUS_DONE, timeout=20)
        # discard unsaved laps so later tests start clean
        self.client.emit('discard_laps')
        wait_for_race_status(self, RACE_STATUS_READY, timeout=20)


class ConfigTest(harness.ProtocolTestCase):
    def test_config_load_limiting(self):
        # set_config does not itself broadcast config_update; the UI
        # re-requests config via load_data. Upstream restricts which config
        # sections a page may fetch (RHUI.CONFIG_LOAD_ITEMS): GENERAL is
        # limited to SECONDARIES/SERIAL_PORTS, SENSORS allows the whole
        # section, and SECRETS is never fetchable.
        self.client.emit('set_config', {
            'section': 'GENERAL',
            'key': 'RACE_START_DELAY_EXTRA_SECS',
            'value': 0.5,
        })
        time.sleep(0.5)
        # an item outside the GENERAL allowlist is not served
        self.client.emit('load_data', {'load_types': [
            {'type': 'config', 'value': {'GENERAL': ['RACE_START_DELAY_EXTRA_SECS']}}]})
        resp = self.client.wait_for('config_update', timeout=15)
        config = resp[0]['config']
        self.assertNotIn('RACE_START_DELAY_EXTRA_SECS', config.get('GENERAL', {}))
        # allowed GENERAL items are served
        self.client.emit('load_data', {'load_types': [
            {'type': 'config', 'value': {'GENERAL': ['SERIAL_PORTS']}}]})
        resp = self.client.wait_for('config_update', timeout=15)
        config = resp[0]['config']
        self.assertIn('GENERAL', config)
        self.assertIn('SERIAL_PORTS', config['GENERAL'])
        # a reserved section is never served
        self.client.emit('load_data', {'load_types': [
            {'type': 'config', 'value': {'SECRETS': ['adminPassword']}}]})
        resp = self.client.wait_for('config_update', timeout=15)
        config = resp[0]['config']
        self.assertNotIn('SECRETS', config)

    def test_set_option_visible_via_http(self):
        # note: set_option does not broadcast option_update; option state is
        # fetched via the HTTP options endpoint
        self.client.emit('set_option', {
            'option': 'eventDescription',
            'value': 'protocol-test ' + _TEST_TOKEN,
        })
        time.sleep(0.5)
        opts = self.client.http_get_json('/api/options')
        self.assertEqual(opts['options'].get('eventDescription'), 'protocol-test ' + _TEST_TOKEN)

    def test_http_api_options(self):
        opts = self.client.http_get_json('/api/options')
        self.assertIn('options', opts)


if __name__ == '__main__':
    unittest.main()