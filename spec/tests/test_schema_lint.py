"""CI guards for cross-spec newline regexes and spaced CDDL bounds."""
import unittest
from pathlib import Path
import tempfile

from schema_lint import lint


class SchemaLintTests(unittest.TestCase):
    def test_repository_schema_lexical_guards(self):
        self.assertEqual(lint(Path(__file__).resolve().parents[2]), [])

    def test_unwitnessed_pattern_fails_closed(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / 'spec').mkdir()
            (root / 'spec/impossible.schema.json').write_text(r'{"pattern":"(?!)"}')
            self.assertEqual(lint(root)[0]['kind'], 'pattern-no-positive-witness')

    def test_json_pattern_trailing_newline(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / 'spec').mkdir()
            path = root / 'spec/unsafe.schema.json'
            path.write_text('{"pattern":"^[a-z]+$"}')
            findings = lint(root)
            self.assertEqual(len(findings), 1)
            self.assertEqual(findings[0]['kind'], 'pattern-trailing-newline')
            self.assertIn('example', findings[0])
            path.write_text(r'{"pattern":"^[a-z]+(?![\\s\\S])"}')
            self.assertEqual(lint(root), [])

    def test_spaced_occurrence_bounds_and_lexical_exclusions(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / 'spec').mkdir()
            path = root / 'spec/test.cddl'
            path.write_text('root = {* 0 tstr => tstr}\n')
            self.assertEqual(lint(root)[0]['kind'], 'spaced-cddl-bound')
            path.write_text('root = {*0 tstr => tstr}\n; ignore [* 17 x]\nx = "literal * 2"\n')
            self.assertEqual(lint(root), [])

    def test_generated_output_is_checked(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / 'sdk').mkdir()
            (root / 'sdk/generated.py').write_text('SOURCE = "root = [* 17 uint]"\n')
            self.assertEqual(lint(root)[0]['kind'], 'spaced-cddl-bound')


if __name__ == '__main__':
    unittest.main()
