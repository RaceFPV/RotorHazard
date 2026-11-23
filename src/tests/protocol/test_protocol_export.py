'''
Data export/import round-trip protocol tests.

Exports a "JSON (Complete) / All" payload, wipes pilots, then imports it
back and verifies pilots/heats/slots are restored (with heat-node pilot
remapping).  Also covers the exporter/importer listings.
'''
import json
import os
import sys
import time
import unittest

try:
    import harness
except ImportError:
    sys.path.append(os.path.dirname(os.path.realpath(__file__)))
    import harness

COMPLETE_ALL = 'JSON__Complete____All'
IMPORTER_NAME = 'RotorHazard_4_0_JSON__Complete_'  # cleanVarName of the label


class ExportImportTest(unittest.TestCase):
    """Owns a dedicated fresh server: the import path is sensitive to
    shared-server state (aged heats/classes), so a clean instance keeps the
    round-trip deterministic on both implementations."""

    @classmethod
    def setUpClass(cls):
        cls.server = harness.spawn_custom_server()
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

    def setUp(self):
        self.tc.client.drain()
        self.tc.observer.drain()

    def emit_and_wait(self, *args, **kwargs):
        return self.tc.emit_and_wait(*args, **kwargs)

    def wait_any(self, *args, **kwargs):
        return self.tc.wait_any(*args, **kwargs)

    def wait_for_new_id(self, *args, **kwargs):
        return self.tc.wait_for_new_id(*args, **kwargs)

    def test_exporter_and_importer_listings(self):
        resp = self.emit_and_wait('load_data', {'load_types': ['exporter_list', 'importer_list']},
                                  'exporter_list', drain=True)
        exporters = resp[0]['exporters']
        names = [e['name'] for e in exporters]
        self.assertIn(COMPLETE_ALL, names)
        resp = self.wait_any('importer_list', timeout=10)
        importers = resp[0]['importers']
        self.assertTrue(any(i['name'] == IMPORTER_NAME for i in importers))

    def _build_data(self):
        '''Creates two pilots, a class, and a heat with assignments; returns (heat id, pilot ids).'''
        resp = self.emit_and_wait('add_race_class', {'name': 'EXP Class'}, 'class_data', drain=True)
        class_id = max(c['id'] for c in resp[0]['classes'])
        pilot_ids = []
        for cs in ('EXP A', 'EXP B'):
            resp = self.emit_and_wait('load_data', {'load_types': ['pilot_data']}, 'pilot_data', drain=True)
            prev = max((p['pilot_id'] for p in resp[0]['pilots']), default=0)
            pid = self.wait_for_new_id('add_pilot', None, 'pilot_data', 'pilot_id', 'pilots', prev)
            self.emit_and_wait('alter_pilot', {
                'pilot_id': pid, 'callsign': cs, 'team_name': 'X',
                'phonetic': 'exp', 'name': 'Export ' + cs,
            }, 'pilot_data', drain=True)
            pilot_ids.append(pid)

        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data', drain=True)
        prev = max((h['id'] for h in resp[0]['heats']), default=0)
        heat = self.wait_for_new_id('add_heat', {'class': class_id}, 'heat_data', 'id', 'heats', prev)
        self.emit_and_wait('alter_heat', {'heat': heat, 'name': 'EXP Heat'}, 'heat_data', drain=True)
        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data')
        heat_row = next(h for h in resp[0]['heats'] if h['id'] == heat)
        for slot, pid in zip(heat_row['slots'][:2], pilot_ids):
            self.emit_and_wait('alter_heat', {'heat': heat, 'slot_id': slot['id'], 'pilot': pid},
                               'heat_data', drain=True)
        return heat, pilot_ids

    def test_export_import_roundtrip(self):
        heat, pilot_ids = self._build_data()

        # export complete data
        resp = self.emit_and_wait('export_database', {'exporter': COMPLETE_ALL},
                                  'exported_data', timeout=20, drain=True)
        payload = resp[0]
        self.assertEqual(payload['encoding'], 'application/json')
        self.assertTrue(payload['filename'].endswith('.json'))
        exported = json.loads(payload['data'])
        self.assertIn('Pilot', exported)
        self.assertIn('Heat', exported)
        self.assertIn('HeatNode', exported)

        # wipe pilots and the heat, then import the export back
        for pid in pilot_ids:
            self.tc.client.emit('delete_pilot', {'pilot': pid})
        time.sleep(0.5)
        self.tc.client.emit('delete_heat', {'heat': heat})
        time.sleep(0.5)

        resp = self.emit_and_wait('import_data', {
            'importer': IMPORTER_NAME,
            'source_data': payload['data'],
            'params': {'reset_pilots': True, 'reset_formats': False,
                       'reset_profiles': False, 'reset_classes': False},
        }, 'database_restore_done', timeout=30, drain=True)

        # pilots restored with the same callsigns
        resp = self.emit_and_wait('load_data', {'load_types': ['pilot_data']}, 'pilot_data', drain=True)
        callsigns = {p['callsign'] for p in resp[0]['pilots']}
        self.assertIn('EXP A', callsigns)
        self.assertIn('EXP B', callsigns)

        # heat restored (by note name) with slots remapped to the new pilots
        resp = self.emit_and_wait('load_data', {'load_types': ['heat_data']}, 'heat_data', drain=True)
        heats = resp[0]['heats']
        restored = next((h for h in heats if h['name'] is not None and 'EXP Heat' in str(h['name'])), None)
        self.assertIsNotNone(restored, 'imported heat missing: ' + json.dumps(heats, default=str)[:600])
        slot_pilots = [s['pilot_id'] for s in restored['slots'][:2]]
        self.assertTrue(all(pid in pilot_ids for pid in slot_pilots if pid is not None),
                        'slot pilots not remapped: ' + json.dumps(restored['slots'][:2]))


if __name__ == '__main__':
    unittest.main()