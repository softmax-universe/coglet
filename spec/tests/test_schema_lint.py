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

    def test_fixed_large_repeat_uses_actual_witness(self):
        import json
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / 'spec').mkdir()
            path = root / 'spec/large.schema.json'
            path.write_text(json.dumps({'pattern': r'^[a-f]{128}(?![\s\S])',
                                        'examples': ['a' * 128]}))
            self.assertEqual(lint(root), [])
            path.write_text(json.dumps({'pattern': r'^[a-f]{128}$',
                                        'examples': ['a' * 128]}))
            self.assertEqual(lint(root)[0]['kind'], 'pattern-trailing-newline')
            path.write_text(json.dumps({'pattern': r'^[a-f]{128}(?![\s\S])'}))
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

    def test_intrinsic_lf_is_not_an_anchor_bypass(self):
        import json
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / 'spec').mkdir()
            path = root / 'spec/path.schema.json'
            abi = json.loads((Path(__file__).resolve().parents[1] /
                              'messages/abi.schema.json').read_text())
            pattern = abi['$defs']['path']['pattern']
            path.write_text(json.dumps({'pattern': pattern,
                                        'examples': ['/data/count', '/data/co\nunt']}))
            self.assertEqual(lint(root), [])

    def test_unanchored_partial_match_is_a_bypass(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / 'spec').mkdir()
            (root / 'spec/partial.schema.json').write_text('{"pattern":"abc"}')
            self.assertEqual(lint(root)[0]['kind'], 'pattern-trailing-newline')

    def test_spaced_occurrence_bounds_and_lexical_exclusions(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / 'spec').mkdir()
            path = root / 'spec/test.cddl'
            path.write_text('root = {* 0 tstr => tstr}\n')
            self.assertEqual(lint(root)[0]['kind'], 'spaced-cddl-bound')
            path.write_text('root = {*0 tstr => tstr}\n; ignore [* 17 x]\nx = "literal * 2"\n')
            self.assertEqual(lint(root), [])

    def test_actual_generated_output_paths_are_checked(self):
        # All current outputs of codegen.generate.generate, including Java IR.
        paths = [
            'crates/coglet-types/src/generated.rs',
            'sdk/surface-python/coglet_surface/generated.py',
            'sdk/surface-go/types/generated.go',
            'sdk/surface-java/src/main/java/org/coglet/surface/Generated.java',
            'sdk/surface-java/src/main/resources/schema.json',
            'sdk/surface-swift/Sources/CogletTypes/Generated/Types.swift',
        ]
        for relative in paths:
            with self.subTest(path=relative), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                path = root / relative
                path.parent.mkdir(parents=True)
                path.write_text('"root = [* 17 uint]"\n')
                self.assertEqual(lint(root), [{
                    'file': relative, 'kind': 'spaced-cddl-bound',
                    'line': 1, 'token': '* 17',
                }])
                path.write_text('"root = [*17 uint]"\n')
                self.assertEqual(lint(root), [])

    def test_generated_output_is_checked(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / 'sdk').mkdir()
            (root / 'sdk/generated.py').write_text('SOURCE = "root = [* 17 uint]"\n')
            self.assertEqual(lint(root)[0]['kind'], 'spaced-cddl-bound')


if __name__ == '__main__':
    unittest.main()
