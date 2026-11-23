'''python -m unittest discover'''
import os
import sys
import unittest

# self-bootstrap so this module is importable regardless of load order
#  (unittest discovery is alphabetical; 'test_Sensors' sorts before
#  'test_server', which would otherwise set up the search paths)
_TESTS_DIR = os.path.dirname(os.path.realpath(__file__))
_SRC_DIR = os.path.dirname(_TESTS_DIR)
for _path in (os.path.join(_SRC_DIR, 'server'), os.path.join(_SRC_DIR, 'server', 'util'), os.path.join(_SRC_DIR, 'interface'), _TESTS_DIR):
    if _path not in sys.path:
        sys.path.append(_path)

from Sensors import Sensors