'''Unit tests for RHUtils helper functions.'''
import os
import sys
import unittest

_TESTS_DIR = os.path.dirname(os.path.realpath(__file__))
_SRC_DIR = os.path.dirname(_TESTS_DIR)
for _path in (os.path.join(_SRC_DIR, 'server'), os.path.join(_SRC_DIR, 'server', 'util'), os.path.join(_SRC_DIR, 'interface'), _TESTS_DIR):
    if _path not in sys.path:
        sys.path.append(_path)

import RHUtils  #pylint: disable=import-error


class FormatTimeToStrTest(unittest.TestCase):
    def test_zero(self):
        self.assertEqual(RHUtils.format_time_to_str(0), '0:00.000')

    def test_positive(self):
        self.assertEqual(RHUtils.format_time_to_str(61001), '1:01.001')

    def test_large(self):
        self.assertEqual(RHUtils.format_time_to_str(3723456), '62:03.456')

    def test_negative(self):
        self.assertEqual(RHUtils.format_time_to_str(-1500), '-0:01.500')

    def test_rounding(self):
        self.assertEqual(RHUtils.format_time_to_str(1499.6), '0:01.500')

    def test_non_numeric(self):
        self.assertEqual(RHUtils.format_time_to_str(None), '')
        self.assertEqual(RHUtils.format_time_to_str('abc'), '')

    def test_custom_format(self):
        self.assertEqual(RHUtils.format_time_to_str(61001, '{m}:{s}'), '1:01')


class FormatSplitTimeToStrTest(unittest.TestCase):
    def test_leading_zeros_removed(self):
        self.assertEqual(RHUtils.format_split_time_to_str(61001), '1:01.001')
        self.assertEqual(RHUtils.format_split_time_to_str(1500), '1.500')

    def test_sub_second(self):
        self.assertEqual(RHUtils.format_split_time_to_str(500), '0.500')

    def test_non_numeric(self):
        self.assertEqual(RHUtils.format_split_time_to_str(None), '')


class FormatPhoneticTimeToStrTest(unittest.TestCase):
    def test_no_minutes(self):
        # phonetic format floors to tenths; note the format template leaves a
        # leading space when minutes are empty -- characterized as-is
        self.assertEqual(RHUtils.format_phonetic_time_to_str(15349), ' 15.3')

    def test_with_minutes(self):
        self.assertEqual(RHUtils.format_phonetic_time_to_str(61000), '1 01.0')

    def test_non_numeric(self):
        self.assertEqual(RHUtils.format_phonetic_time_to_str(None), '')


class DurationStrTest(unittest.TestCase):
    def test_format(self):
        self.assertEqual(RHUtils.format_secs_to_duration_str(61.0), '1:01.0')
        self.assertEqual(RHUtils.format_secs_to_duration_str(0), '0:00.0')
        self.assertEqual(RHUtils.format_secs_to_duration_str(61.123), '1:01.123')

    def test_parse_roundtrip(self):
        for val in (0.0, 1.5, 61.0, 123.456, 3600.0):
            dur_str = RHUtils.format_secs_to_duration_str(val)
            self.assertAlmostEqual(RHUtils.parse_duration_str_to_secs(dur_str), val, places=3)

    def test_parse_errors(self):
        self.assertIsNone(RHUtils.parse_duration_str_to_secs(''))
        self.assertIsNone(RHUtils.parse_duration_str_to_secs('not-a-time'))


class VersionStrTest(unittest.TestCase):
    def test_check(self):
        self.assertTrue(RHUtils.checkVersionStr('3.9', 3, 8))
        self.assertTrue(RHUtils.checkVersionStr('3.9', 3, 9))
        self.assertFalse(RHUtils.checkVersionStr('3.9', 3, 10))
        # characterized quirk: a higher major version does not satisfy the
        # minor-version check ('4.0' vs required 3.10 returns False)
        self.assertFalse(RHUtils.checkVersionStr('4.0', 3, 10))


class FindPrefixedSubstringTest(unittest.TestCase):
    def test_found(self):
        data = b'Version: 1.2.3\x00other'
        self.assertEqual(RHUtils.findPrefixedSubstring(data, 'Version: ', 32), '1.2.3')

    def test_missing_terminator(self):
        self.assertEqual(RHUtils.findPrefixedSubstring(b'Version: 1.2.3', 'Version: ', 32), '1.2.3')

    def test_too_long(self):
        self.assertIsNone(RHUtils.findPrefixedSubstring(b'Version: ' + b'x' * 64 + b'\0', 'Version: ', 32))

    def test_not_found(self):
        self.assertIsNone(RHUtils.findPrefixedSubstring(b'nope', 'Version: ', 32))


class UniqueNameTest(unittest.TestCase):
    def test_no_conflict(self):
        self.assertEqual(RHUtils.uniqueName('Name', ['Other']), 'Name')

    def test_single_conflict(self):
        self.assertEqual(RHUtils.uniqueName('Name', ['Name']), 'Name 2')

    def test_repeated_conflicts(self):
        self.assertEqual(RHUtils.uniqueName('Name', ['Name', 'Name 2']), 'Name 3')

    def test_from_base(self):
        self.assertEqual(RHUtils.unique_name_from_base('Heat', ['Heat']), 'Heat 1')


class FilenameHelpersTest(unittest.TestCase):
    def test_append_to_base(self):
        self.assertEqual(RHUtils.appendToBaseFilename('file.db', '_bkp'), 'file_bkp.db')
        self.assertEqual(RHUtils.appendToBaseFilename('file', '_bkp'), 'file_bkp')

    def test_clean_var_name(self):
        self.assertEqual(RHUtils.cleanVarName('my var-name!'), 'my_var_name_')

    def test_clean_file_name(self):
        # each disallowed character becomes one '-'; only spaces/dots at the
        # ends are stripped, so the interior '.' survives
        self.assertEqual(RHUtils.cleanFileName('  bad:name?.db '), 'bad-name-.db')


class ColorHelpersTest(unittest.TestCase):
    def test_hsl_known_values(self):
        # red at h=0, s=100, l=50
        self.assertEqual(RHUtils.hslToHex(0, 100, 50), '#ff0000')
        # green at h=120
        self.assertEqual(RHUtils.hslToHex(120, 100, 50), '#00ff00')

    def test_hsl_grayscale(self):
        # regression: the s==0 (grayscale) branch used to crash on float
        # formatting; exercise it explicitly
        self.assertEqual(RHUtils.hslToHex(0, 0, 0), '#000000')
        self.assertEqual(RHUtils.hslToHex(0, 0, 100), '#ffffff')
        self.assertEqual(RHUtils.hslToHex(0, 0, 50), '#808080')

    def test_hsl_randomized_stays_in_range(self):
        for _ in range(20):
            hexstr = RHUtils.hslToHex(None, None, None)
            self.assertTrue(hexstr.startswith('#'))
            self.assertEqual(len(hexstr), 7)

    def test_hex_to_color(self):
        self.assertEqual(RHUtils.hexToColor('#ff0000'), 0xff0000)


class GetNumericEntryTest(unittest.TestCase):
    def test_numeric(self):
        self.assertEqual(RHUtils.getNumericEntry({'a': 5}, 'a'), 5)
        self.assertEqual(RHUtils.getNumericEntry({'a': 2.5}, 'a'), 2.5)

    def test_defaults(self):
        self.assertEqual(RHUtils.getNumericEntry({'a': 'x'}, 'a'), 0)
        self.assertEqual(RHUtils.getNumericEntry({'a': 'x'}, 'a', 7), 7)
        self.assertEqual(RHUtils.getNumericEntry(None, 'a'), 0)


if __name__ == '__main__':
    unittest.main()