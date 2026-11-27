"""Source-policy checks, not native compilation or event-delivery proof."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
APP = (ROOT / 'Sources/SoftmaxApp/SoftmaxApp.swift').read_text()
VIEW = (ROOT / 'Sources/SoftmaxApp/Windows.swift').read_text()
UI = (ROOT / 'Tests/SoftmaxUITests/SoftmaxUITests.swift').read_text()
METHOD = UI.split('func testSettingsFocusedSearchArrowNavigation() {', 1)[1].split('\n    @MainActor', 1)[0]
COLLECTOR = APP.split('enum SettingsArrowCapture {', 1)[1].split('\n@MainActor\nfunc settingsArrowCapture', 1)[0]


class SettingsCapturePolicyTests(unittest.TestCase):
    def test_gate_and_lazy_recording(self):
        record = COLLECTOR.split('static func record(', 1)[1].split('static func exportBeforeEscape', 1)[0]
        self.assertIn('#if DEBUG', record)
        self.assertIn('arguments.contains("--ui-testing")', COLLECTOR)
        self.assertIn('environment["SOFTMAX_SETTINGS_ARROW_PROBE"]', COLLECTOR)
        self.assertLess(record.index('guard outputURL != nil, !exported'), record.index('let value = metadata()'))
        self.assertLess(record.index('guard records.count < 1024'), record.index('let value = metadata()'))
        self.assertIn('reserveCapacity(1024)', COLLECTOR)
        self.assertIn('dropped += 1', record)

    def test_only_final_escape_writes(self):
        record = COLLECTOR.split('static func record(', 1)[1].split('static func exportBeforeEscape', 1)[0]
        self.assertNotIn('.write(', record)
        self.assertNotIn('FileHandle', APP)
        self.assertEqual(COLLECTOR.count('.write('), 1)
        self.assertIn('try data.write(to: outputURL, options: .atomic)', COLLECTOR)
        self.assertIn('if event.keyCode == 53 { SettingsArrowCapture.exportBeforeEscape() }', APP)
        self.assertEqual(APP.count('SettingsArrowCapture.exportBeforeEscape()'), 1)
        self.assertNotIn('async', COLLECTOR)

    def test_record_privacy_allowlist(self):
        record = COLLECTOR.split('struct Record: Codable {', 1)[1].split('\n    }', 1)[0]
        fields = set(re.findall(r'let (\w+):', record))
        self.assertEqual(fields, {'sequence', 'uptime', 'phase', 'active', 'hasKeyWindow', 'responder',
                                  'selected', 'previous', 'keyCode', 'eventTime', 'queryEmpty', 'matchCount', 'searchFocused'})
        self.assertNotIn('characters', COLLECTOR)
        self.assertNotIn('.title', COLLECTOR)
        self.assertIn('.prefix(128)', COLLECTOR)
        self.assertNotIn('settingsArrowProbe', APP + VIEW)
        self.assertNotIn('print(', METHOD)

    def test_original_keyboard_sequence(self):
        calls = re.findall(r'settings\.(typeText\([^\n]*?\)|typeKey\([^\n]*?\))', METHOD)
        self.assertEqual(calls, [
            'typeText("/")', 'typeText("o")',
            'typeKey(.downArrow, modifierFlags: [])', 'typeKey(.return, modifierFlags: [])',
            'typeKey(.downArrow, modifierFlags: [])', 'typeKey(.return, modifierFlags: [])',
            'typeKey(.upArrow, modifierFlags: [])', 'typeKey(.return, modifierFlags: [])',
            'typeKey("a", modifierFlags: .command)', 'typeText("zz-no-matches")',
            'typeKey(.downArrow, modifierFlags: [])', 'typeKey(.upArrow, modifierFlags: [])',
            'typeKey(.return, modifierFlags: [])', 'typeKey(.escape, modifierFlags: [])'])
        self.assertEqual(re.findall(r'input\("(\w+)"\)', METHOD), [
            'slash', 'filter', 'firstDown', 'firstReturn', 'secondDown', 'secondReturn', 'up', 'upReturn',
            'selectAll', 'noMatches', 'emptyDown', 'emptyUp', 'emptyReturn', 'escape'])
        self.assertNotIn('sleep', METHOD)
        self.assertNotIn('XCTSkip', METHOD)
        self.assertEqual(re.findall(r'timeout: (\d+)', METHOD), ['10', '5'])

    def test_original_assertions_and_order(self):
        assertions = re.findall(r'XCTAssertTrue\([^\n]+', METHOD)
        self.assertEqual(assertions, [
            'XCTAssertTrue(onboarding.waitForExistence(timeout: 10))',
            'XCTAssertTrue(settings.waitForExistence(timeout: 5))',
            'XCTAssertTrue(settings.buttons["General"].exists)',
            'XCTAssertTrue(settings.buttons["Hotkey"].exists)',
            'XCTAssertTrue(settings.buttons["Audio"].exists)',
            'XCTAssertTrue(settings.buttons["Record Hotkey…"].exists, "Focused search Down must select Hotkey")',
            'XCTAssertTrue(settings.checkBoxes["Play sound cues"].exists, "Second Down must select Audio")',
            'XCTAssertTrue(settings.buttons["Record Hotkey…"].exists, "Up must return to Hotkey")',
            'XCTAssertTrue(settings.staticTexts["No matching settings"].exists)',
            'XCTAssertTrue(settings.buttons["Record Hotkey…"].exists, "Empty search must retain the current section")',
            'XCTAssertTrue(settings.buttons["General"].exists)'])

    def test_capture_completeness_and_cleanup(self):
        self.assertIn('defer { app.terminate(); try? FileManager.default.removeItem(at: probe) }', METHOD)
        self.assertIn('driver.count == 28', METHOD)
        self.assertIn('driver.count < 32', METHOD)
        self.assertIn('records.last?["keyCode"] as? Int == 53', METHOD)
        self.assertIn('let dropped = document?["dropped"] as? Int', METHOD)
        self.assertIn('(dropped ?? 0) > 0 ? "overflow"', METHOD)
        self.assertIn('valid && dropped == 0 ? "complete"', METHOD)
        self.assertIn('XCTAssertEqual(status, "complete"', METHOD)
        self.assertIn('attachment.name = "settings-arrow-boundaries.json"', METHOD)
        self.assertIn('attachment.lifetime = .keepAlways', METHOD)

    def test_selection_and_view_boundaries(self):
        self.assertIn('SettingsArrowCapture.record(.selectionSet)', APP)
        self.assertIn('.init(selected: selectedTab, previous: oldValue)', APP)
        for phase in ('downEnter', 'downExit', 'upEnter', 'upExit', 'submitEnter', 'submitExit',
                      'viewAppear', 'viewDisappear', 'viewSelection', 'viewQuery', 'viewFocus'):
            self.assertIn(f'capture(.{phase})', VIEW)


if __name__ == '__main__':
    unittest.main()
