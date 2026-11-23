"""Registry coverage fails closed before the compiler can skip an input."""
import json
from pathlib import Path
import tempfile
import unittest
from contracts import load, schema_paths, fixture_paths


class ContractRegistryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for directory in ('codegen', 'spec/messages', 'spec/fixtures/common', 'spec/fixtures/draft'):
            (self.root / directory).mkdir(parents=True, exist_ok=True)
        self.registry = {'common': 'generated', 'draft': 'draft-validated'}
        self.save()
        for name in self.registry:
            (self.root / f'spec/messages/{name}.cddl').write_text('rule = tstr')
            (self.root / f'spec/fixtures/{name}/cases.json').write_text('[]')

    def save(self):
        (self.root / 'codegen/contracts.json').write_text(json.dumps(self.registry))

    def test_selection_and_nested_discovery(self):
        nested = self.root / 'spec/fixtures/common/nested'
        nested.mkdir()
        (nested / 'more.json').write_text('[]')
        self.assertEqual([p.name for p in schema_paths(self.root)], ['common.cddl'])
        self.assertEqual(len(fixture_paths(self.root)), 2)
        self.assertEqual(load(self.root), self.registry)

    def test_unclassified_schema(self):
        (self.root / 'spec/messages/new.cddl').write_text('new = tstr')
        with self.assertRaisesRegex(ValueError, 'unclassified.*schemas'):
            schema_paths(self.root)

    def test_unclassified_fixture_directory(self):
        (self.root / 'spec/fixtures/new').mkdir()
        with self.assertRaisesRegex(ValueError, 'unclassified.*fixtures'):
            fixture_paths(self.root)

    def test_root_fixture(self):
        (self.root / 'spec/fixtures/rogue.json').write_text('[]')
        with self.assertRaisesRegex(ValueError, 'root fixture'):
            load(self.root)

    def test_stale_entry(self):
        self.registry['stale'] = 'generated'
        self.save()
        with self.assertRaisesRegex(ValueError, 'stale schemas'):
            load(self.root)

    def test_invalid_mode(self):
        self.registry['draft'] = 'skip'
        self.save()
        with self.assertRaisesRegex(ValueError, 'classification'):
            load(self.root)

    def test_duplicate_entry(self):
        (self.root / 'codegen/contracts.json').write_text('{"common":"generated","common":"draft-validated"}')
        with self.assertRaisesRegex(ValueError, 'duplicate contract'):
            load(self.root)

    def test_empty_family(self):
        (self.root / 'spec/fixtures/draft/cases.json').unlink()
        with self.assertRaisesRegex(ValueError, 'empty fixture'):
            load(self.root)

    def test_root_schema_and_ambiguous_stem(self):
        (self.root / 'spec/messages/draft.cddl').rename(self.root / 'spec/draft.cddl')
        self.assertEqual(load(self.root), self.registry)
        (self.root / 'spec/messages/draft.cddl').write_text('rule = tstr')
        with self.assertRaisesRegex(ValueError, 'ambiguous'):
            load(self.root)


if __name__ == '__main__':
    unittest.main()
