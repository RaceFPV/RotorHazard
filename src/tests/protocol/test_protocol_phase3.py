'''
Phase 3 protocol tests: heat generation (calc/reset/confirm), language
switching in rendered pages, page auth gating, and callouts.
'''
import base64
import json
import os
import sys
import time
import unittest
import urllib.request

try:
    import harness
except ImportError:
    sys.path.append(os.path.dirname(os.path.realpath(__file__)))
    import harness



def wait_config_value(server, section, key, expected, timeout=6.0):
    """Polls config.json until it holds the expected value (set_config
    persists asynchronously relative to other tests).  Skips when the server
    handle has no data directory (RH_TEST_URL mode)."""
    if not getattr(server, "data_dir", None):
        time.sleep(1.0)
        return
    deadline = time.monotonic() + timeout
    path = os.path.join(server.data_dir, "config.json")
    while time.monotonic() < deadline:
        try:
            cfg = json.load(open(path))
            if str(cfg.get(section, {}).get(key)) == str(expected):
                return
        except (OSError, ValueError):
            pass
        time.sleep(0.2)
    raise AssertionError("config {}.{} never became {!r}".format(section, key, expected))


class HeatPlanTest(harness.ProtocolTestCase):
    def _build_heat(self, num_pilots=2):
        pilot_ids = []
        for _ in range(num_pilots):
            resp = self.emit_and_wait('load_data', {'load_types': ['pilot_data']}, 'pilot_data', drain=True)
            prev = max((p['pilot_id'] for p in resp[0]['pilots']), default=0)
            pilot_ids.append(self.wait_for_new_id('add_pilot', None, 'pilot_data',
                                                  'pilot_id', 'pilots', prev))

        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data', drain=True)
        prev = max((h['id'] for h in resp[0]['heats']), default=0)
        heat = self.wait_for_new_id('add_heat', None, 'heat_data', 'id', 'heats', prev)
        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data')
        heat_row = next(h for h in resp[0]['heats'] if h['id'] == heat)
        for slot, pid in zip(heat_row['slots'][:num_pilots], pilot_ids):
            self.emit_and_wait('alter_heat', {'heat': heat, 'slot_id': slot['id'], 'pilot': pid},
                               'heat_data', drain=True)
        return heat

    def test_calc_confirm_reset_cycle(self):
        heat = self._build_heat()

        # characterized: calc on a static heat (assigned pilots, no dynamic
        # slots, no auto-frequency) returns 'safe' -- no heat_plan_result at all
        self.client.drain()
        self.observer.drain()
        self.client.emit('calc_pilots', {'heat': heat})
        time.sleep(1.0)
        for cl in (self.client, self.observer):
            with cl._cond:
                self.assertNotIn('heat_plan_result', [n for n, _ in cl._events])

        # confirm the plan
        resp = self.emit_and_wait('confirm_heat_plan', {'heat_id': heat}, 'heat_data', timeout=15, drain=True)
        heat_row = next(h for h in resp[0]['heats'] if h['id'] == heat)
        self.assertEqual(heat_row['status'], 2)  # CONFIRMED

        # reset back to planned (also silent for a static heat)
        self.client.emit('calc_reset', {'heat': heat})
        time.sleep(0.5)
        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data', drain=True)
        heat_row = next(h for h in resp[0]['heats'] if h['id'] == heat)
        self.assertEqual(heat_row['status'], 0)  # PLANNED

    def test_seeded_slot_calculation(self):
        """Dynamic slot seeding: a heat slot seeded from a previous heat's
        results gets the winning pilot on calc."""
        # build heat A with 2 pilots and save a race for it
        heat_a = self._build_heat()
        resp = self.emit_and_wait('load_data', {'load_types': ['pilot_data']}, 'pilot_data', drain=True)
        pilot_ids = sorted(p['pilot_id'] for p in resp[0]['pilots'])[-2:]

        resp = self.emit_and_wait('load_data', {'load_types': ['format_data']}, 'format_data', drain=True)
        prev = max((f['id'] for f in resp[0]['formats']), default=0)
        fmt = self.wait_for_new_id('add_race_format', {'source_format_id': 1},
                                   'format_data', 'id', 'formats', prev, timeout=15)
        self.emit_and_wait('alter_race_format', {
            'format_id': fmt, 'format_name': 'SeedFmt', 'race_time_sec': 60,
            'lap_grace_sec': 60, 'staging_fixed_tones': 1, 'staging_delay_tones': 0,
            'start_delay_min_ms': 100, 'start_delay_max_ms': 300,
            'number_laps_win': 0, 'win_condition': 1, 'unlimited_time': 0,
        }, 'format_data', timeout=15)
        self.emit_and_wait('set_race_format', {'race_format': fmt}, 'race_status', timeout=15)
        self.emit_and_wait('set_current_heat', {'heat': heat_a}, 'current_heat', drain=True)

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
        for node in (0, 1):
            self.client.emit('simulate_lap', {'node': node})
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
        self.client.emit('save_laps')
        self.wait_any('race_saved', timeout=20, drain=True)

        # heat B with a slot seeded from heat A result rank 1
        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data', drain=True)
        prev = max((h['id'] for h in resp[0]['heats']), default=0)
        heat_b = self.wait_for_new_id('add_heat', None, 'heat_data', 'id', 'heats', prev)
        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data')
        slot = next(h for h in resp[0]['heats'] if h['id'] == heat_b)['slots'][0]
        self.emit_and_wait('alter_heat', {
            'heat': heat_b, 'slot_id': slot['id'], 'method': 1,
            'seed_heat_id': heat_a, 'seed_rank': 1,
        }, 'heat_data', drain=True)

        # calc emits the plan and seeds the slot with heat A's winner
        resp = self.emit_and_wait('calc_pilots', {'heat': heat_b}, 'heat_plan_result', timeout=15, drain=True)
        payload = resp[0]
        self.assertTrue(payload['calc_result']['has_calc_pilots'])
        seeded = payload['slots'][0]
        self.assertIn(seeded['pilot_id'], pilot_ids,
                      'seeded pilot not from heat A: ' + json.dumps(payload['slots'][:2]))
        self.assertIsNotNone(seeded['callsign'])


class LanguageTest(harness.ProtocolTestCase):
    def test_language_switch_affects_pages(self):
        # switch to German; rendered page titles must translate
        self.client.emit('set_language', {'language': 'de'})
        wait_config_value(self.server, 'UI', 'currentLanguage', 'de')
        with urllib.request.urlopen(self.server.base_url + '/', timeout=10) as resp:
            body = resp.read().decode('utf-8', errors='replace')
        self.assertIn('Startseite', body, 'German translation not applied to home page')

        # full language dictionary is available
        resp = self.emit_and_wait('load_data', {'load_types': ['all_languages']}, 'all_languages', drain=True)
        langs = resp[0]['languages']
        self.assertIn('de', langs)
        self.assertEqual(langs['de']['name'], 'Deutsch')

        # switch back to English
        self.client.emit('set_language', {'language': 'EN'})
        wait_config_value(self.server, 'UI', 'currentLanguage', 'EN')
        with urllib.request.urlopen(self.server.base_url + '/', timeout=10) as resp:
            body = resp.read().decode('utf-8', errors='replace')
        self.assertIn('Home', body)
        self.assertNotIn('Startseite', body)


class PageAuthTest(unittest.TestCase):
    '''Page auth gating on a dedicated server with credentials configured.'''

    @classmethod
    def setUpClass(cls):
        cls.server = harness.spawn_custom_server({
            'SECRETS': {'ADMIN_USERNAME': 'authu', 'ADMIN_PASSWORD': 'authp'},
        })

    @classmethod
    def tearDownClass(cls):
        cls.server.stop()

    def test_authed_page_gating(self):
        try:
            urllib.request.urlopen(self.server.base_url + '/marshal', timeout=10)
            self.fail('expected 401 for unauthenticated /marshal')
        except urllib.error.HTTPError as ex:
            self.assertEqual(ex.code, 401)
            self.assertIn('Basic', ex.headers.get('WWW-Authenticate', ''))

        req = urllib.request.Request(self.server.base_url + '/marshal')
        creds = base64.b64encode(b'authu:authp').decode()
        req.add_header('Authorization', 'Basic ' + creds)
        with urllib.request.urlopen(req, timeout=10) as resp:
            self.assertEqual(resp.status, 200)

    def test_public_pages_open(self):
        with urllib.request.urlopen(self.server.base_url + '/', timeout=10) as resp:
            self.assertEqual(resp.status, 200)
        with urllib.request.urlopen(self.server.base_url + '/results', timeout=10) as resp:
            self.assertEqual(resp.status, 200)


class CalloutTest(harness.ProtocolTestCase):
    def test_callout_save_reload_and_expansion(self):
        callouts = ['%LAP_COUNT% is live', 'Next heat %HEAT_NAME%']

        # save on one client, reload from the other (save is noself)
        self.client.emit('save_callouts', {'callouts': callouts})
        time.sleep(0.5)
        resp = self.observer.emit_and_wait('reload_callouts', None, 'callouts', timeout=15, drain=True)
        self.assertEqual(resp[0], callouts)

        # token expansion: recognized tokens are cleared without pilot data;
        # unrecognized tokens survive (characterized)
        resp = self.emit_and_wait('get_callout_text', {'callout': '%LAP_COUNT% %EVENT_NAME%'},
                                  'callout_text', timeout=15, drain=True)
        self.assertEqual(resp[0]['callout'], '%LAP_COUNT% %EVENT_NAME%')
        self.assertEqual(resp[0]['text'], ' %EVENT_NAME%')




class MockProviderTest(unittest.TestCase):
    """Upstream provider architecture: MOCK_NODE_SIGNAL names a signal
    provider (none / replay / simulated variants); the server switches
    live without a restart."""

    @classmethod
    def setUpClass(cls):
        cls.server = harness.spawn_rust_server()
        cls.tc = harness.ProtocolTestCase.__new__(harness.ProtocolTestCase)
        cls.tc.server = cls.server
        cls.tc.client = harness.Client(cls.server.base_url)
        cls.tc.observer = harness.Client(cls.server.base_url)
        cls.tc.client.connect()
        cls.tc.observer.connect()

    @classmethod
    def tearDownClass(cls):
        cls.tc.client.disconnect()
        cls.tc.observer.disconnect()
        cls.server.stop()

    def _collect_rssi(self, seconds=2.5):
        vals = []
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                args = self.tc.wait_any("heartbeat", timeout=1)
                vals.extend(args[0]["current_rssi"])
            except AssertionError:
                break
        return vals

    def test_signal_none_freezes_rssi(self):
        # characterized: NoSignal returns no reading, so nodes keep their
        # last RSSI (frozen) rather than going to zero.  Wait out the
        # provider transition (any prior test's readings must flush), then
        # require a constant stream.
        self.tc.client.emit("set_config", {
            "section": "GENERAL", "key": "MOCK_NODE_SIGNAL", "value": "0"})
        wait_config_value(self.server, "GENERAL", "MOCK_NODE_SIGNAL", "0")
        time.sleep(1.0)  # let the provider switch take effect on the tick

        def collect_arrays(seconds):
            # drain the backlog: without this the queue serves stale
            # heartbeats buffered during earlier waits
            self.tc.client.drain()
            self.tc.observer.drain()
            arrs = []
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                try:
                    args = self.tc.wait_any("heartbeat", timeout=0.4)
                    arrs.append(args[0]["current_rssi"])
                except AssertionError:
                    break
            return arrs

        window_a = collect_arrays(1.5)
        window_b = collect_arrays(1.5)
        self.assertTrue(window_a and window_b, "no heartbeat readings")
        # frozen: per-node values identical across both windows (each node
        # keeps its OWN last reading -- they are not all equal to each other)
        n = len(window_a[-1])
        for idx in range(n):
            vals = [hb[idx] for hb in (window_a + window_b) if len(hb) > idx]
            self.assertTrue(all(v == vals[0] for v in vals),
                            "node {} not frozen: {}".format(idx, vals))

    def test_signal_selection_on_advanced_settings(self):
        import base64
        req = urllib.request.Request(self.server.base_url + "/advanced-settings")
        creds = base64.b64encode(b"admin:rotorhazard").decode()
        # this instance has empty configured credentials; auth disabled
        with urllib.request.urlopen(req, timeout=10) as resp:
            body = resp.read().decode("utf-8", errors="replace")
        self.assertIn("Simulated - Pass Peak Only", body)
        self.assertIn("mock_data_[n].csv", body)
        self.assertIn("Mock Node Update Interval", body)

    def test_replay_csv_signal_registers_laps(self):
        # write a replay file for node 1: readings every 0.5s of recorded
        # time; lap_id advances twice with completed crossings
        lines = []
        lap_id = 1
        for i in range(12):
            read_time = i * 0.5
            cross = "T" if i % 4 in (1, 2) else "F"
            if i in (4, 8):
                lap_id += 1
            rssi = 120 if cross == "T" else 30
            lines.append(",".join(str(x) for x in [
                read_time, lap_id, 0, rssi, 100, 130, 1, cross, 20, 20,
                rssi, 0, 0, 20, 0, 0]))
        data_dir = self.server.data_dir
        with open(os.path.join(data_dir, "mock_data_1.csv"), "w") as f:
            f.write("\n".join(lines) + "\n")

        # switch to replay; the server picks it up without a restart
        self.tc.client.emit("set_config", {
            "section": "GENERAL", "key": "MOCK_NODE_SIGNAL", "value": "1"})
        time.sleep(0.5)
        vals = [v for v in self._collect_rssi(3.0)]
        replayed = [v for v in vals if v in (30, 120)]
        self.assertTrue(replayed, "no replayed RSSI values observed")

        # during a race the replayed passes register laps on node 1
        resp = self.tc.emit_and_wait("load_data", {"load_types": ["heat_data"]},
                                     "heat_data", drain=True)
        heat = max((h["id"] for h in resp[0]["heats"]), default=None)
        if heat is None:
            self.tc.emit_and_wait("add_heat", None, "heat_data", drain=True)
            resp = self.tc.emit_and_wait("load_data", {"load_types": ["heat_data"]},
                                         "heat_data")
            heat = max(h["id"] for h in resp[0]["heats"])
        self.tc.emit_and_wait("add_pilot", None, "pilot_data", drain=True)
        resp = self.tc.emit_and_wait("load_data", {"load_types": ["pilot_data"]},
                                     "pilot_data", drain=True)
        pid = max(p["pilot_id"] for p in resp[0]["pilots"])
        resp = self.tc.emit_and_wait("load_data", {"load_types": ["heat_data"]},
                                     "heat_data")
        heat_row = next(h for h in resp[0]["heats"] if h["id"] == heat)
        self.tc.emit_and_wait("alter_heat",
                              {"heat": heat, "slot_id": heat_row["slots"][0]["id"],
                               "pilot": pid}, "heat_data", drain=True)
        self.tc.emit_and_wait("set_current_heat", {"heat": heat}, "current_heat",
                              drain=True)

        self.tc.client.drain()
        self.tc.observer.drain()
        self.tc.client.emit("stage_race")
        deadline = time.monotonic() + 45
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AssertionError("race never started")
            args = self.tc.wait_any("race_status", timeout=remaining)
            if args[0].get("race_status") == 1:
                break

        laps_seen = False
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline and not laps_seen:
            try:
                args = self.tc.wait_any("current_laps",
                                        timeout=max(1, deadline - time.monotonic()))
                node1 = args[0]["current"]["node_index"][0]
                if node1["laps"]:
                    laps_seen = True
            except AssertionError:
                pass
        self.assertTrue(laps_seen, "replayed passes did not register laps")

        self.tc.client.emit("stop_race")
        time.sleep(1)
        self.tc.client.emit("discard_laps")
        # back to the default simulated signal; wait for the persist so a
        # following test cannot race this write
        self.tc.client.emit("set_config", {
            "section": "GENERAL", "key": "MOCK_NODE_SIGNAL", "value": "2"})
        wait_config_value(self.server, "GENERAL", "MOCK_NODE_SIGNAL", "2")


if __name__ == "__main__":
    unittest.main()

class DefaultConfigTest(unittest.TestCase):
    """First-run behavior: the server creates config.json with default
    admin/rotorhazard credentials and gates authed pages accordingly."""

    @classmethod
    def setUpClass(cls):
        cls.server = harness.spawn_rust_server(write_config=False)

    @classmethod
    def tearDownClass(cls):
        cls.server.stop()

    def test_default_config_and_auth(self):
        cfg = json.load(open(os.path.join(self.server.data_dir, "config.json")))
        self.assertEqual(cfg["SECRETS"]["ADMIN_USERNAME"], "admin")
        self.assertEqual(cfg["SECRETS"]["ADMIN_PASSWORD"], "rotorhazard")
        self.assertTrue(cfg["SECRETS"]["SECRET_KEY"])
        self.assertEqual(cfg["GENERAL"]["MOCK_NODE_SIGNAL"], 2)

        try:
            urllib.request.urlopen(self.server.base_url + "/marshal", timeout=10)
            self.fail("expected 401")
        except urllib.error.HTTPError as ex:
            self.assertEqual(ex.code, 401)
        req = urllib.request.Request(self.server.base_url + "/marshal")
        req.add_header("Authorization", "Basic " +
                       base64.b64encode(b"admin:rotorhazard").decode())
        with urllib.request.urlopen(req, timeout=10) as resp:
            self.assertEqual(resp.status, 200)

        req = urllib.request.Request(self.server.base_url + "/advanced-settings")
        req.add_header("Authorization", "Basic " +
                       base64.b64encode(b"admin:rotorhazard").decode())
        with urllib.request.urlopen(req, timeout=10) as resp:
            self.assertEqual(resp.status, 200)

        client = harness.Client(self.server.base_url)
        client.connect()
        try:
            resp = client.emit_and_wait("load_data",
                                        {"load_types": ["pilot_data"]},
                                        "admin_creds_defaulted", timeout=15, drain=True)
            self.assertEqual(resp[0]["username"], "admin")
        finally:
            client.disconnect()


class MockSignalTest(unittest.TestCase):
    """Mock node RSSI simulation (MOCK_NODE_SIGNAL=2): heartbeat rssi values
    vary, crossing flags appear, and laps register automatically during a
    running race."""

    @classmethod
    def setUpClass(cls):
        os.environ["RH_MOCK_CROSSING_PCT"] = "60"
        env_spawn = {"GENERAL": {"MOCK_NODE_SIGNAL": 2}}
        cls.server = harness.spawn_rust_server(env_spawn)
        cls.tc = harness.ProtocolTestCase.__new__(harness.ProtocolTestCase)
        cls.tc.server = cls.server
        cls.tc.client = harness.Client(cls.server.base_url)
        cls.tc.observer = harness.Client(cls.server.base_url)
        cls.tc.client.connect()
        cls.tc.observer.connect()

    @classmethod
    def tearDownClass(cls):
        os.environ.pop("RH_MOCK_CROSSING_PCT", None)
        cls.tc.client.disconnect()
        cls.tc.observer.disconnect()
        cls.server.stop()

    def _wait_racing(self):
        deadline = time.monotonic() + 45
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AssertionError("race never started")
            args = self.tc.wait_any("race_status", timeout=remaining)
            if args[0].get("race_status") == 1:
                return

    def test_mock_rssi_varies_and_laps_register(self):
        seen = set()
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline and len(seen) < 3:
            args = self.tc.wait_any("heartbeat", timeout=2)
            for v in args[0]["current_rssi"]:
                if v:
                    seen.add(v)
        self.assertGreaterEqual(len(seen), 3, "rssi not varying")
        for v in seen:
            self.assertTrue(20 <= v < 150, "rssi out of range")

        # pilots must be assigned to heat seats for crossings to count
        resp = self.tc.emit_and_wait("load_data", {"load_types": ["pilot_data"]},
                                     "pilot_data", drain=True)
        pilot_ids = [p["pilot_id"] for p in resp[0]["pilots"]]
        resp = self.tc.emit_and_wait("load_data", {"load_types": ["heat_data"]},
                                     "heat_data", drain=True)
        heat = max((h["id"] for h in resp[0]["heats"]), default=None)
        if heat is None:
            self.tc.emit_and_wait("add_heat", None, "heat_data", drain=True)
            resp = self.tc.emit_and_wait("load_data", {"load_types": ["heat_data"]},
                                         "heat_data")
            heat = max(h["id"] for h in resp[0]["heats"])
        resp = self.tc.emit_and_wait("load_data", {"load_types": ["heat_data"]},
                                     "heat_data")
        heat_row = next(h for h in resp[0]["heats"] if h["id"] == heat)
        for slot, pid in zip(heat_row["slots"][:2], pilot_ids[:2]):
            self.tc.emit_and_wait("alter_heat",
                                  {"heat": heat, "slot_id": slot["id"], "pilot": pid},
                                  "heat_data", drain=True)
        self.tc.emit_and_wait("set_current_heat", {"heat": heat}, "current_heat",
                              drain=True)

        self.tc.client.drain()
        self.tc.observer.drain()
        self.tc.client.emit("stage_race")
        self._wait_racing()

        saw_crossing = False
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            args = self.tc.wait_any("heartbeat", timeout=max(1, deadline - time.monotonic()))
            if any(args[0]["crossing_flag"]):
                saw_crossing = True
                break
        self.assertTrue(saw_crossing, "no crossing flags in mock simulation")

        laps_seen = False
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline and not laps_seen:
            try:
                args = self.tc.wait_any("current_laps",
                                        timeout=max(1, deadline - time.monotonic()))
                for node in args[0]["current"]["node_index"]:
                    if node["laps"]:
                        laps_seen = True
            except AssertionError:
                pass
        self.assertTrue(laps_seen, "no automatic laps during race")

        self.tc.client.emit("stop_race")
        time.sleep(1)
        self.tc.client.emit("discard_laps")
