"""Source-policy checks for the AutoFill heuristics opt-out (not native proof).

Native proof is testSettingsSearchDoesNotStartAutoFillHelper; these checks keep
the registration at the process entry, ungated, and the original arrow test intact.
"""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
APP = (ROOT / 'Sources/SoftmaxApp/SoftmaxApp.swift').read_text()
UI = (ROOT / 'Tests/SoftmaxUITests/SoftmaxUITests.swift').read_text()
REGISTER = 'UserDefaults.standard.register(defaults: ["NSAutoFillHeuristicsEnabled": false])'


class AutoFillPolicyTests(unittest.TestCase):
    def entry(self):
        return APP.split('@main\nenum SoftmaxEntry {', 1)[1].split('\n}\n', 1)[0]

    def test_registered_once_at_process_entry_before_app(self):
        entry = self.entry()
        self.assertEqual(APP.count(REGISTER), 1)
        self.assertIn(REGISTER, entry)
        self.assertLess(entry.index(REGISTER), entry.index('SoftmaxApp.main()'))
        self.assertEqual(APP.count('@main'), 1)
        self.assertNotIn('@main\nstruct SoftmaxApp', APP)

    def test_not_gated_on_testing_or_debug(self):
        code = re.sub(r'//.*', '', self.entry())
        for gate in ('#if', 'ui-testing', 'testing', 'ProcessInfo'):
            self.assertNotIn(gate, code)

    def test_regression_has_positive_control_and_product_phase(self):
        method = UI.split('func testSettingsSearchDoesNotStartAutoFillHelper() {', 1)[1].split('\n    }\n', 1)[0]
        self.assertIn('"-NSAutoFillHeuristicsEnabled", "YES"', method)
        self.assertIn('phase("autoFillEnabled", enableAutoFill: true)', method)
        self.assertIn('phase("productDefault", enableAutoFill: false)', method)
        self.assertIn('XCTAssertFalse(enabled.helpers.isEmpty', method)
        self.assertIn('XCTAssertTrue(product.helpers.isEmpty', method)
        self.assertIn('XCTAssertTrue(product.hotkey', method)
        # Only the enabled (positive-control) branch polls between 'o' and Down.
        between = method.split('let filterEnd = ProcessInfo.processInfo.systemUptime', 1)[1].split(
            'settings.typeKey(.downArrow', 1)[0]
        enabled_branch, rest = between.split('if enableAutoFill {', 1)[1].split('\n            }\n', 1)
        self.assertIn('current()', enabled_branch)
        self.assertNotIn('current()', rest)
        self.assertNotIn('waitUntil', rest)

    def test_original_arrow_test_unchanged_assertions(self):
        method = UI.split('func testSettingsFocusedSearchArrowNavigation() {', 1)[1].split('\n    }\n', 1)[0]
        for message in ('Focused search Down must select Hotkey', 'Second Down must select Audio',
                        'Up must return to Hotkey', 'Empty search must retain the current section'):
            self.assertIn(message, method)
        self.assertNotIn('AutoFill', method)


if __name__ == '__main__':
    unittest.main()
