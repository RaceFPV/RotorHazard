'''
Protocol tests for the features added during the Rust conversion:
the calibration suggestion socket surface and the cluster (secondary
timer) socket surface.  Runs against any server the harness provides
(Python baseline or Rust rewrite via RH_TEST_URL).
'''
import json
import time

from harness import ProtocolTestCase
from test_protocol_lifecycle import RaceFlowBase


class ClusterSecondaryHandlersTest(ProtocolTestCase):
    '''A timer joined by a primary must answer the cluster handshake.'''

    def test_join_cluster_ex_gets_response_with_server_info(self):
        args = self.emit_and_wait('join_cluster_ex', {'mode': 'split'},
                                  'join_cluster_response')
        payload = args[0]
        self.assertIn('server_info', payload)
        info = json.loads(payload['server_info'])
        self.assertIn('release_version', info)
        # with no admin credentials configured, auth is open
        self.assertIs(payload.get('auth_ok'), True)

    def test_check_secondary_query_gets_timestamped_response(self):
        args = self.emit_and_wait('check_secondary_query', {'timestamp': 0},
                                  'check_secondary_response')
        payload = args[0]
        self.assertIn('timestamp', payload)
        ts = payload['timestamp']
        self.assertIsInstance(ts, (int, float))
        # the timestamp must be an epoch-milliseconds value (near "now")
        now_ms = time.time() * 1000
        self.assertLess(abs(now_ms - ts), 60000,
                        'check_secondary_response timestamp not near current epoch ms')


class CalibrationSuggestionTest(RaceFlowBase):
    '''The marshal-page suggestion socket surface (needs a race-ready heat
    so staging actually starts the race).'''

    def _restore_mock_signal(self):
        self.client.emit('set_config', {'section': 'GENERAL',
                                        'key': 'MOCK_NODE_SIGNAL', 'value': '0'})

    def test_suggest_without_history_reports_no_history(self):
        # establish a clean slate: a race cycle with idle mocks (signal 0)
        # clears per-node history at race start and records nothing
        self.make_race_ready(num_pilots=2)
        self.stage_and_wait_racing()
        self.emit_and_wait('stop_race', {}, 'race_status', timeout=10)
        self.client.emit('discard_laps', {})
        time.sleep(0.3)
        args = self.emit_and_wait('suggest_calibration', {'node': 0},
                                  'calibration_suggestion')
        payload = args[0]
        self.assertEqual(payload.get('node'), 0)
        self.assertIsNone(payload.get('pilotrace_id'))
        self.assertIsNone(payload.get('enter_at'))
        self.assertIsNone(payload.get('exit_at'))
        self.assertEqual(payload.get('reason'), 'no_history')

    def test_suggest_with_simulated_history_returns_result(self):
        try:
            # switch to the simulated provider, which records peak/nadir history
            self.client.emit('set_config', {'section': 'GENERAL',
                                            'key': 'MOCK_NODE_SIGNAL', 'value': '2'})
            time.sleep(0.3)
            # run a race long enough that (with ~5% crossing odds per 0.5s
            # reading) essentially every seat records at least one pass
            self.make_race_ready(num_pilots=2)
            self.stage_and_wait_racing()
            time.sleep(22)
            self.emit_and_wait('stop_race', {}, 'race_status', timeout=10)
            with_history = 0
            for node in range(8):
                args = self.emit_and_wait('suggest_calibration', {'node': node},
                                          'calibration_suggestion', drain=True)
                payload = args[0]
                self.assertIn('enter_at', payload)
                self.assertIn('exit_at', payload)
                self.assertIn('lap_count', payload)
                self.assertIn('reason', payload)
                # seats with a frequency record history; the Python seed
                # disables a node in the default profile, so 'no_history'
                # is legitimate for frequency-less seats on either seed
                if payload.get('reason') == 'no_history':
                    continue
                self.assertNotEqual(payload.get('reason'), 'pass_peak_only')
                if payload.get('reason') is None:
                    with_history += 1
                    self.assertIsInstance(payload['enter_at'], int)
                    self.assertIsInstance(payload['exit_at'], int)
                    self.assertIsInstance(payload['lap_count'], int)
                    self.assertGreater(payload['enter_at'], payload['exit_at'])
                else:
                    # seats without recorded gate passes report it explicitly
                    self.assertIn(payload.get('reason'),
                                  ('weak_signal', 'no_gate_passes', 'no_clean_exit'))
            # random per-seat crossings: virtually all 8 seats see passes in 22s
            self.assertGreaterEqual(with_history, 3,
                                    'too few seats produced calibration suggestions')
        finally:
            self._restore_mock_signal()
            # clear the mock race so later tests start clean
            self.client.emit('discard_laps', {})
            time.sleep(0.3)

    def test_suggest_unknown_seat_reports_not_found(self):
        args = self.emit_and_wait('suggest_calibration', {'node': 999},
                                  'calibration_suggestion')
        payload = args[0]
        self.assertEqual(payload.get('reason'), 'not_found')