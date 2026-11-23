'''Unit tests for pure-logic modules under src/server/util/.'''
import os
import sys
import unittest

_TESTS_DIR = os.path.dirname(os.path.realpath(__file__))
_SRC_DIR = os.path.dirname(_TESTS_DIR)
for _path in (os.path.join(_SRC_DIR, 'server'), os.path.join(_SRC_DIR, 'server', 'util'), os.path.join(_SRC_DIR, 'interface'), _TESTS_DIR):
    if _path not in sys.path:
        sys.path.append(_path)

from RunningMedian import RunningMedian  #pylint: disable=import-error


class RunningMedianTest(unittest.TestCase):
    def test_single_value(self):
        rm = RunningMedian(3)
        rm.insert(5)
        self.assertEqual(rm.median(), 5)

    def test_odd_window_median(self):
        rm = RunningMedian(5)
        for val in (3, 1, 2):
            rm.insert(val)
        self.assertEqual(rm.median(), 2)

    def test_even_window_median(self):
        rm = RunningMedian(4)
        for val in (4, 1, 3, 2):
            rm.insert(val)
        self.assertEqual(rm.median(), 2.5)

    def test_window_slides(self):
        rm = RunningMedian(3)
        for val in (10, 20, 30):
            rm.insert(val)
        self.assertEqual(rm.median(), 20)
        rm.insert(40)  # 10 slides out; window is now 20,30,40
        self.assertEqual(rm.median(), 30)

    def test_replacement_behavior(self):
        # inserting at capacity replaces the oldest value, not grows
        rm = RunningMedian(2)
        rm.insert(1)
        rm.insert(3)
        self.assertEqual(rm.median(), 2)
        rm.insert(100)  # replaces 1 -> window is 3,100
        self.assertEqual(rm.median(), 51.5)

    def test_duplicates(self):
        rm = RunningMedian(4)
        for val in (5, 5, 5, 5):
            rm.insert(val)
        self.assertEqual(rm.median(), 5)
        rm.insert(1)
        self.assertEqual(rm.median(), 5)

    def test_monotonic_sequence(self):
        rm = RunningMedian(5)
        for val in range(1, 11):
            rm.insert(val)
        self.assertEqual(rm.median(), 8)

    def test_matches_naive_implementation(self):
        import RunningMedian as rm_module  #pylint: disable=import-error
        from random import Random
        rng = Random(1234)
        rm = RunningMedian(7)
        naive = rm_module.NaiveRunningMedian(7)
        for _ in range(200):
            val = rng.randint(0, 1000)
            rm.insert(val)
            naive.insert(val)
            self.assertEqual(rm.median(), naive.median())


if __name__ == '__main__':
    unittest.main()