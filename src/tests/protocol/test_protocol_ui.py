'''
UI page-serving tests: every main page returns 200 and includes the shared
layout markers (rotorhazard.js wiring), so both implementations must serve
a working frontend shell.
'''
import os
import sys
import unittest
import urllib.request

try:
    import harness
except ImportError:
    sys.path.append(os.path.dirname(os.path.realpath(__file__)))
    import harness

PAGES = ['/', '/event', '/results', '/run', '/current', '/marshal', '/format',
         '/settings', '/scanner', '/streams', '/database']


class UiPageTest(harness.ProtocolTestCase):
    def _get(self, path):
        # like a browser: retry with the default admin credentials when the
        # page requires authentication
        import base64
        creds = base64.b64encode(b'admin:rotorhazard').decode()
        try:
            with urllib.request.urlopen(self.server.base_url + path, timeout=10) as resp:
                return resp.status, resp.read().decode('utf-8', errors='replace')
        except urllib.error.HTTPError as ex:
            if ex.code != 401:
                raise
            req = urllib.request.Request(self.server.base_url + path)
            req.add_header('Authorization', 'Basic ' + creds)
            with urllib.request.urlopen(req, timeout=10) as resp:
                return resp.status, resp.read().decode('utf-8', errors='replace')

    def test_pages_render(self):
        for path in PAGES:
            status, body = self._get(path)
            self.assertEqual(status, 200, 'GET {} failed'.format(path))
            # layout markers: socket.io client + shared layout branding
            self.assertIn('socket.io', body, '{} missing socket.io wiring'.format(path))
            self.assertIn('RotorHazard', body, '{} missing layout shell'.format(path))

    def test_static_assets_served(self):
        status, _ = self._get('/static/jquery-3.6.4/jquery-3.6.4.min.js')
        self.assertEqual(status, 200)


if __name__ == '__main__':
    unittest.main()