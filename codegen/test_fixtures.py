import json
from pathlib import Path
import tempfile
import unittest

from fixtures import cases, shape_valid


class DiscoveryTests(unittest.TestCase):
    def test_marker_is_explicit_and_strict(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'vectors.json'
            path.write_text('{"signatures": []}')
            with self.assertRaisesRegex(ValueError, 'case array'):
                cases(path)
            marker = path.with_name(path.name + '.format')
            marker.write_text('semantic-vectors-v1\n')
            self.assertIsNone(cases(path))
            marker.write_text('unknown')
            with self.assertRaisesRegex(ValueError, 'unknown fixture format'):
                cases(path)
            marker.write_text('semantic-vectors-v1')
            path.write_text('{')
            with self.assertRaises(ValueError):
                cases(path)

    def test_case_semantics_and_malformed_entries(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'cases.json'
            case = dict(name='signature-invalid', type='event', valid=False, semantic=True, value={})
            path.write_text(json.dumps([case]))
            self.assertTrue(shape_valid(cases(path)[0]))
            case['semantic'] = False
            path.write_text(json.dumps([case]))
            self.assertFalse(shape_valid(cases(path)[0]))
            for data in ([], [case, case], [dict(name='bad', type='event', value={})],
                         [dict(case, valid='false')], [dict(case, semantic='true')]):
                path.write_text(json.dumps(data))
                with self.subTest(data=data), self.assertRaises(ValueError):
                    cases(path)
