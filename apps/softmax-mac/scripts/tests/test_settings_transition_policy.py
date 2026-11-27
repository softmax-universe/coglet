"""Finite producer source contracts only; native qualification is separate."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
PROBE = (ROOT / 'Sources/SoftmaxApp/SettingsTransitionProbe.swift').read_text()
APP = (ROOT / 'Sources/SoftmaxApp/SoftmaxApp.swift').read_text()
VIEW = (ROOT / 'Sources/SoftmaxApp/Windows.swift').read_text()
UI = (ROOT / 'Tests/SoftmaxUITests/SoftmaxUITests.swift').read_text()


class SettingsTransitionPolicyTests(unittest.TestCase):
    def test_debug_and_explicit_gates(self):
        self.assertTrue(PROBE.startswith('#if DEBUG\n'))
        self.assertTrue(PROBE.rstrip().endswith('#endif'))
        for gate in ('--ui-testing', 'SOFTMAX_SETTINGS_TRANSITION_SCENARIO', 'SOFTMAX_SETTINGS_TRANSITION_OUTPUT'):
            self.assertIn(gate, PROBE)
        for text in ('SettingsTransitionProbe.attach', 'SettingsTransitionProbe.arrival'):
            self.assertRegex(APP, r'#if DEBUG\n[^#]*' + re.escape(text))
        for text in ('SettingsTransitionProbe.queryChanged', 'SettingsTransitionProbe.notificationReceived'):
            self.assertRegex(VIEW, r'#if DEBUG\n[^#]*' + re.escape(text))

    def test_exact_finite_scenarios(self):
        cases = PROBE.split('enum Scenario: String, Codable { case ', 1)[1].split(' }', 1)[0]
        self.assertEqual(cases, 'settled, queuedAfterSlash, queuedAfterQuery')
        self.assertIn('for scenario in ["settled", "queuedAfterSlash", "queuedAfterQuery"]', UI)
        self.assertIn('if label == .slash, scenario == .queuedAfterSlash, !finished { post([.queryO, .firstDown]) }', PROBE)
        self.assertIn('armed = false; post([.firstDown])', PROBE)
        query_hook = PROBE.split('static func queryChanged', 1)[1].split('static func notificationReceived', 1)[0]
        self.assertNotIn('NSApp.sendEvent(', query_hook)

    def test_bounds_and_export_separation(self):
        for value in ('scenarioWall = 15.0', 'capacity = 128'):
            self.assertIn(value, PROBE)
        tick = PROBE.split('private static func tick()', 1)[1].split('private static func run', 1)[0]
        self.assertLess(tick.index('SettingsProbeDeadline.expired(now: now'), tick.index('let done = run'))
        self.assertLess(tick.index('let done = run'), tick.index('let completedAt = ProcessInfo'))
        self.assertLess(tick.index('SettingsProbeDeadline.expired(now: completedAt'), tick.index('if done {'))
        self.assertIn('let schema = 2', PROBE)
        self.assertIn('let boundary = "transitionScenarioEnd"', PROBE)
        self.assertEqual(PROBE.count('.write('), 1)
        self.assertIn('timer?.invalidate(); timer = nil', PROBE)
        self.assertIn('let boundary = "finalEscapeBeforeDispatch"', APP)
        self.assertNotIn('exportBeforeEscape()', PROBE)

    def test_no_focus_or_selection_injection(self):
        executable = '\n'.join(line.split('//')[0] for line in PROBE.splitlines())
        for forbidden in ('makeFirstResponder(', 'fieldEditor(', '.keyDown(', 'doCommand(', 'selectedTab =', 'sleep(', 'RunLoop.run'):
            self.assertNotIn(forbidden, executable)
        self.assertIn('guard let editor = fields[0].currentEditor(), let responder = window.firstResponder', PROBE)
        self.assertIn('return editor === responder', PROBE)
        self.assertIn('NSApp.sendEvent($0)', PROBE)
        self.assertIn('NSApp.postEvent($0, atStart: false)', PROBE)

    def test_failure_requires_qualified_route_and_handler(self):
        self.assertIn('let preconditionsMet = firstArrived && firstOwned == true && firstReady', PROBE)
        self.assertIn('outcome = .preconditionUnavailable', PROBE)
        self.assertIn('outcome = .invalidRoute', PROBE)
        self.assertIn('guard actual != tab || !handlerCorrect else { return }', PROBE)
        for kind in ('missingHandler', 'wrongSelection', 'laterOverwrite'):
            self.assertIn(kind, PROBE)
        self.assertIn('SettingsArrowCapture.records.count - 1', PROBE)

    def test_evidence_privacy(self):
        fields = PROBE.split('struct Checkpoint: Codable {', 1)[1].split('\n    }', 1)[0]
        self.assertEqual(set(re.findall(r'let (\w+):', fields)),
                         {'sequence', 'uptime', 'kind', 'label', 'editorOwned', 'selected', 'matchCount'})
        self.assertNotIn('print(', PROBE)
        self.assertNotIn('window.title', PROBE)
        self.assertNotIn('String(describing:', PROBE)

    def test_driver_closed_on_invalid_evidence(self):
        method = UI.split('func testSettingsTransitionOrderEvidence() throws {', 1)[1].split('\n    @MainActor', 1)[0]
        self.assertIn('XCTAssertEqual(outcome, "pass"', method)
        self.assertIn('["pass", "transitionOnly"].contains(outcome)', method)
        self.assertIn('document["complete"] as? Bool, true', method)
        self.assertIn('document["dropped"] as? Int, 0', method)
        self.assertNotIn('XCTSkip', method)
        self.assertNotIn('continueAfterFailure = false', method)


if __name__ == '__main__':
    unittest.main()
