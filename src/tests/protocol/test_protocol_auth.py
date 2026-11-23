'''
Authentication protocol tests.

Spawns a dedicated server with admin credentials configured (socket auth
enabled) and verifies both the rejection of unauthenticated actions and the
acceptance of authenticated ones.  Separate from the shared-server suites
because auth state is global to a server instance.
'''
import time
import unittest
import urllib.request

try:
    import harness
except ImportError:  # direct discovery from the protocol directory
    import os
    import sys
    sys.path.append(os.path.dirname(os.path.realpath(__file__)))
    import harness

ADMIN_USER = 'testadmin'
ADMIN_PASS = 'testpass'


class AuthTest(unittest.TestCase):
    '''Owns its own server instance with auth enabled.'''

    @classmethod
    def setUpClass(cls):
        cls.server = harness.spawn_custom_server({
            'SECRETS': {
                'ADMIN_USERNAME': ADMIN_USER,
                'ADMIN_PASSWORD': ADMIN_PASS,
            },
        })

    @classmethod
    def tearDownClass(cls):
        cls.server.stop()

    def test_unauthenticated_socket_action_rejected(self):
        client = harness.Client(self.server.base_url)
        client.connect()
        try:
            client.drain()
            client.emit('add_pilot')
            time.sleep(1.0)
            # no 'pilot_data' response may arrive for the unauthenticated client
            with self.assertRaises(AssertionError):
                client.wait_for('pilot_data', timeout=3)
        finally:
            client.disconnect()

    def test_authenticated_socket_action_accepted(self):
        # auth travels as HTTP basic auth on the socket connection request
        import requests
        session = requests.Session()
        session.auth = (ADMIN_USER, ADMIN_PASS)
        client = harness.Client(self.server.base_url, http_session=session)
        client.connect()
        try:
            client.drain()
            resp = client.emit_and_wait('add_pilot', None, 'pilot_data', timeout=15)
            self.assertIn('pilots', resp[0])
        finally:
            client.disconnect()

    def test_http_json_api_open_by_design(self):
        # characterized contract: the /api/* JSON endpoints are NOT
        # auth-protected (even with admin credentials configured); only
        # Socket.IO actions are gated.  The rewrite must preserve this.
        with urllib.request.urlopen(self.server.base_url + '/api/pilot/all', timeout=10) as resp:
            self.assertEqual(resp.status, 201)
            body = resp.read().decode('utf-8')
        self.assertIn('pilots', body)


if __name__ == '__main__':
    unittest.main()