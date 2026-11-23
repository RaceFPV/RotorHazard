'''Unit tests for core server logic: event manager, interface mapping,
ack queue, and time helpers.'''
import os
import sys
import unittest
from datetime import datetime

_TESTS_DIR = os.path.dirname(os.path.realpath(__file__))
_SRC_DIR = os.path.dirname(_TESTS_DIR)
for _path in (os.path.join(_SRC_DIR, 'server'), os.path.join(_SRC_DIR, 'server', 'util'), os.path.join(_SRC_DIR, 'interface'), _TESTS_DIR):
    if _path not in sys.path:
        sys.path.append(_path)

import gevent.event  # noqa: F401  (eventmanager expects gevent.event preloaded)
import gevent.queue   # noqa: F401  (SendAckQueue expects gevent.queue preloaded)
from types import SimpleNamespace
import eventmanager  #pylint: disable=import-error
from eventmanager import Evt, EventManager  #pylint: disable=import-error
import RHTimeFns  #pylint: disable=import-error


import gevent  # after submodule preloads; needed for hub yields


class EventManagerTest(unittest.TestCase):
    def setUp(self):
        # EventManager only touches racecontext for event logging config
        stub_rc = SimpleNamespace(serverconfig=SimpleNamespace(
            get_item=lambda *args, **kwargs: 0))
        self.events = EventManager(racecontext=stub_rc)
        self.seen = []

    # handler dispatch contract: priority < 100 runs synchronously inside
    # trigger(); priority >= 100 is spawned as a greenlet (async)

    def test_sync_handler_runs_immediately(self):
        self.events.on(Evt.RACE_START, 't1', lambda args: self.seen.append(args['x']), {}, 50)
        self.events.trigger(Evt.RACE_START, {'x': 7})
        self.assertEqual(self.seen, [7])

    def test_async_handler_runs_after_yield(self):
        self.events.on(Evt.RACE_START, 't1', lambda args: self.seen.append(args['x']), {}, 200)
        self.events.trigger(Evt.RACE_START, {'x': 7})
        self.assertEqual(self.seen, [])  # not yet dispatched
        gevent.sleep(0.1)
        self.assertEqual(self.seen, [7])

    def test_multiple_handlers_fire_in_priority_order(self):
        self.events.on(Evt.RACE_START, 'second', lambda args: self.seen.append('second'), {}, 50)
        self.events.on(Evt.RACE_START, 'first', lambda args: self.seen.append('first'), {}, 1)
        self.events.trigger(Evt.RACE_START, {})
        # ascending priority order, both synchronous
        self.assertEqual(self.seen, ['first', 'second'])

    def test_off_removes_handler(self):
        self.events.on(Evt.RACE_START, 't1', lambda args: self.seen.append(1), {}, 50)
        self.events.off(Evt.RACE_START, 't1')
        self.events.trigger(Evt.RACE_START, {})
        self.assertEqual(self.seen, [])

    def test_handler_exception_does_not_stop_others(self):
        self.events.on(Evt.RACE_START, 'bad', lambda args: 1 // 0, {}, 50)
        self.events.on(Evt.RACE_START, 'good', lambda args: self.seen.append('ok'), {}, 51)
        self.events.trigger(Evt.RACE_START, {})
        self.assertEqual(self.seen, ['ok'])

    def test_trigger_unknown_event_is_noop(self):
        self.events.trigger('not-an-event', {})

    def test_all_event_wildcard(self):
        self.events.on(Evt.ALL, 'wild', lambda args: self.seen.append(args['_eventName']), {}, 50)
        self.events.trigger(Evt.RACE_START, {})
        self.assertEqual(self.seen, ['raceStart'])  # Evt values are camelCase
        self.events.trigger(Evt.HEARTBEAT, {})  # HEARTBEAT skips ALL handlers
        self.assertEqual(self.seen, ['raceStart'])


class _DummyNode:
    def __init__(self, idx):
        self.index = idx
        self.provider = None
        self.provider_type = None


class _DummyInterface:
    def __init__(self, num_nodes):
        self.nodes = [_DummyNode(i) for i in range(num_nodes)]


class InterfaceMapperTest(unittest.TestCase):
    def setUp(self):
        import interface_mapper  #pylint: disable=import-error
        self.module = interface_mapper
        self.mapper = interface_mapper.InterfaceMapper(racecontext=None)

    def test_add_interface_maps_nodes_in_order(self):
        intf_a = _DummyInterface(3)
        intf_b = _DummyInterface(2)
        self.mapper.add_interface(intf_a, self.module.InterfaceType.MOCK, {'api_level': 5})
        self.mapper.add_interface(intf_b, self.module.InterfaceType.RH, None)
        self.assertEqual(len(self.mapper.nodes), 5)
        # provider and type recorded per node
        for node in intf_a.nodes:
            self.assertIs(node.provider, intf_a)
            self.assertEqual(node.provider_type, self.module.InterfaceType.MOCK)
        for node in intf_b.nodes:
            self.assertIs(node.provider, intf_b)
            self.assertEqual(node.provider_type, self.module.InterfaceType.RH)
        # node.index remains the LOCAL index within each interface; global
        # seat numbering is established separately via reindex_nodes()
        self.assertEqual([n.index for n in self.mapper.nodes], [0, 1, 2, 0, 1])

    def test_reindex_nodes_rewrites_global_indexes(self):
        intf = _DummyInterface(3)
        self.mapper.add_interface(intf, self.module.InterfaceType.MOCK, {'api_level': 5})
        intf.nodes[0].index = 99
        self.mapper.reindex_nodes()
        self.assertEqual([n.index for n in self.mapper.nodes], [0, 1, 2])

    def test_remove_nodes_closes_gaps(self):
        intf = _DummyInterface(4)
        self.mapper.add_interface(intf, self.module.InterfaceType.MOCK, {'api_level': 5})
        self.mapper.remove_nodes([intf.nodes[1]])
        self.assertEqual(len(self.mapper.nodes), 3)
        # removal keeps local indexes as-is until an explicit reindex
        self.assertEqual([n.index for n in self.mapper.nodes], [0, 2, 3])
        self.mapper.reindex_nodes()
        self.assertEqual([n.index for n in self.mapper.nodes], [0, 1, 2])

    def test_replace_nodes_swaps_in_place(self):
        intf = _DummyInterface(3)
        self.mapper.add_interface(intf, self.module.InterfaceType.MOCK, {'api_level': 5})
        old_node = intf.nodes[1]
        new_node = _DummyNode(0)
        self.mapper.replace_nodes([old_node], [new_node])
        self.assertNotIn(old_node, self.mapper.nodes)
        self.assertIs(self.mapper.nodes[1], new_node)
        self.assertIs(new_node.provider, intf)


class _FakeSocketIO:
    def __init__(self):
        self.sent = []

    def emit(self, event, payload):
        self.sent.append((event, payload))


class SendAckQueueTest(unittest.TestCase):
    '''Exercises the cluster send/ack queue with a fake socket endpoint.'''

    def test_message_sent_and_cleared_without_ack_expectation(self):
        import gevent
        import SendAckQueue  #pylint: disable=import-error
        fake = _FakeSocketIO()
        queue = SendAckQueue.SendAckQueue(10, fake, unittest_logger)
        queue.put('evt_a', {'k': 1}, waitForAckFlag=False)
        gevent.sleep(0.2)  # let the worker greenlet run
        self.assertEqual(fake.sent, [('evt_a', {'k': 1})])

    def test_message_held_until_acked(self):
        import gevent
        import SendAckQueue  #pylint: disable=import-error
        fake = _FakeSocketIO()
        queue = SendAckQueue.SendAckQueue(10, fake, unittest_logger)
        # before the first ack is ever received, messages are sent and cleared
        queue.put('evt_a', {'k': 1})
        gevent.sleep(0.2)
        self.assertEqual(fake.sent, [('evt_a', {'k': 1})])
        # the first ack arms ack-expectation mode
        queue.ack('evt_a', {'k': 1})
        gevent.sleep(0.1)
        # from now on a waitForAck message stays queued until acked
        queue.put('evt_b', {'k': 2})
        gevent.sleep(0.2)
        self.assertEqual([evt for evt, _ in fake.sent], ['evt_a', 'evt_b'])
        queue.put('evt_c', {'k': 3})
        gevent.sleep(0.2)
        # evt_c waits in the queue while evt_b is still awaiting its ack
        self.assertEqual([evt for evt, _ in fake.sent], ['evt_a', 'evt_b'])
        # acking evt_b releases it and lets evt_c through
        queue.ack('evt_b', {'k': 2})
        gevent.sleep(0.2)
        self.assertEqual([evt for evt, _ in fake.sent], ['evt_a', 'evt_b', 'evt_c'])


class RHTimeFnsTest(unittest.TestCase):
    def test_datetime_format(self):
        dt = datetime(2024, 3, 5, 7, 8, 9, 123000)
        self.assertEqual(RHTimeFns.datetimeToFormattedStr(dt), '2024-03-05 07:08:09.123')

    def test_epoch_ms_format(self):
        # 2024-03-05 07:08:09.123 UTC
        self.assertEqual(RHTimeFns.epochMsToFormattedStr(1709617689123.0),
                         RHTimeFns.datetimeToFormattedStr(datetime.fromtimestamp(1709617689.123)))

    def test_epoch_start(self):
        self.assertEqual(RHTimeFns.getEpochStartTime().year, 1970)


class unittest_logger:  #pylint: disable=too-few-public-methods
    '''Minimal logger stand-in for SendAckQueue.'''
    @staticmethod
    def error(*args, **kwargs):  #pylint: disable=unused-argument
        pass

    @staticmethod
    def warning(*args, **kwargs):  #pylint: disable=unused-argument
        pass

    @staticmethod
    def info(*args, **kwargs):  #pylint: disable=unused-argument
        pass

    @staticmethod
    def exception(*args, **kwargs):  #pylint: disable=unused-argument
        pass


if __name__ == '__main__':
    unittest.main()