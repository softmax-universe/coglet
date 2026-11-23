"""Freeze path safety: schemas and replay semantics, not engine execution."""
import copy
import json
import unittest

from jsonschema import Draft202012Validator
from reference_cddl import Schema, ValidationError
from test_common import SPEC, encode
from test_abi import valid_path, check_record, digest, record_hash


class PathContract(unittest.TestCase):
    def test_all_control_bytes_and_backslash(self):
        common = (SPEC / 'messages/common.cddl').read_text()
        for family, rule in [('abi', 'path'), ('hub', 'hub-write-path')]:
            schema = json.loads((SPEC / f'messages/{family}.schema.json').read_text())
            validator = Draft202012Validator(schema['$defs'][rule])
            cddl = Schema('fixture = ' + rule + '\n' + (SPEC / f'messages/{family}.cddl').read_text() + common)
            for c in [chr(n) for n in range(32)] + ['\x7f', '\\']:
                path = '/data/a' + c + 'b'
                with self.subTest(family=family, byte=ord(c)):
                    self.assertFalse(valid_path(path))
                    self.assertFalse(validator.is_valid(path))
                    with self.assertRaises(ValidationError):
                        cddl.validate_cbor(encode(path))
            for path in ['/data/count', '/data/nested/file-1.txt']:
                self.assertTrue(valid_path(path))
                self.assertTrue(validator.is_valid(path), (family, path, list(validator.iter_errors(path))))
                cddl.validate_cbor(encode(path))

    def test_utf8_is_preserved_and_debug_is_subset(self):
        self.assertTrue(valid_path('/data/caf\u00e9'))
        self.assertTrue(valid_path('/data/cafe\u0301'))
        for path in ['/data/../a', '/data/./a', '/data//a', '/data/a/']:
            self.assertFalse(valid_path(path))

    def test_rehashed_snapshot_control_is_not_hidden_by_hash_failure(self):
        cases = json.loads((SPEC / 'fixtures/abi/cases.json').read_text())
        base = next(c['value'] for c in cases if c['type'] == 'step-record' and c['valid'])
        check_record(base)
        for c in ['\n', '\x01', '\x7f', '\\']:
            rec = copy.deepcopy(base)
            rec['output_files']['/data/a' + c + 'b'] = ''
            rec['output_hash'] = digest(rec['output_files'])
            rec['record_hash'] = record_hash(rec)
            with self.assertRaisesRegex(ValueError, 'unsafe path'):
                check_record(rec)


if __name__ == '__main__':
    unittest.main()
