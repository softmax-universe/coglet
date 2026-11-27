"""Source/privacy policy only; hosted AppKit execution qualifies the census."""
from pathlib import Path
import re
import unittest

APP = Path(__file__).resolve().parents[2] / "Sources" / "SoftmaxApp"


class SettingsCensusPolicyTests(unittest.TestCase):
    def setUp(self):
        self.source = (APP / "SettingsNativeCensus.swift").read_text()

    def test_debug_only(self):
        self.assertTrue(self.source.startswith("#if DEBUG\n"))
        self.assertTrue(self.source.endswith("#endif\n"))

    def test_fixed_vocabulary_schema(self):
        structures = re.findall(r"struct \w+: Codable \{(.*?)\n    \}", self.source, re.S)
        self.assertEqual(len(structures), 3)
        for structure in structures:
            self.assertNotRegex(structure, r":\s*String|Any|NSView|NSText")
        for forbidden in ["String(describing:", "type(of:", "%p", "stringValue", "makeFirstResponder", "fieldEditor(", "layoutSubtreeIfNeeded"]:
            self.assertNotIn(forbidden, self.source)

    def test_all_fields_bounded_without_label_filter(self):
        self.assertIn("visited < 256", self.source)
        self.assertIn("fields.count < 16", self.source)
        self.assertIn("depth >= 32", self.source)
        self.assertIn("256 - visited - stack.count", self.source)
        self.assertIn("children.prefix(remaining)", self.source)
        self.assertIn("walkTruncated", self.source)
        self.assertIn("fieldsTruncated", self.source)
        self.assertIn("if let field = view as? NSTextField {", self.source)

    def test_reserved_roles_and_explicit_missing(self):
        self.assertIn("case waitEditorFirst, viewFocusFirst, terminal", self.source)
        self.assertIn("guard !snapshots.contains(where: { $0.role == role })", self.source)
        self.assertIn("notReached: Role.allCases.filter", self.source)

    def test_identity_is_native_and_read_only(self):
        self.assertIn("field.currentEditor()", self.source)
        self.assertIn("editor === responder", self.source)
        self.assertIn("textView?.delegate === field", self.source)
        self.assertIn('field.placeholderString == "Filter settings  /"', self.source)
        self.assertIn('field.accessibilityLabel() == "Search settings"', self.source)

    def test_probe_hooks_and_gate_unchanged(self):
        probe = (APP / "SettingsTransitionProbe.swift").read_text()
        for role in [".waitEditorFirst", ".terminal", ".viewFocusFirst"]:
            self.assertIn("SettingsNativeCensus.capture(" + role, probe)
        self.assertIn("return ownership() == true", probe)
        # Actual hosted census proved placeholder identity, not SwiftUI's native AX label.
        self.assertIn('field.placeholderString == "Filter settings  /"', probe)
        self.assertIn('return editor === responder', probe)


if __name__ == "__main__":
    unittest.main()
