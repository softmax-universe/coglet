"""Hub 1.0 schemas and spec-only management oracles, not runtime conformance."""
import copy
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest

import cbor2
from jsonschema import Draft202012Validator
from reference_cddl import Schema, ValidationError
from referencing import Registry, Resource
from test_common import SPEC, decode, encode
from manage_oracle import apply, tree, signing, code_path, Rejected, size_check, MAX_PATCH, tail, normalize


def read_frame(raw):
    if len(raw) < 4:
        raise ValueError('short prefix')
    size = struct.unpack('>I', raw[:4])[0]
    if not 1 <= size <= 1048576 or len(raw) != size + 4:
        raise ValueError('length')
    return decode(raw[4:])


class HubFixtures(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.root = json.loads((SPEC / 'messages/hub.schema.json').read_text())
        common = json.loads((SPEC / 'messages/common.schema.json').read_text())
        cls.registry = Registry().with_resource('https://coglet.dev/spec/messages/common.schema.json', Resource.from_contents(common))
        cls.cddl = (SPEC / 'messages/hub.cddl').read_text() + (SPEC / 'messages/common.cddl').read_text()
        cls.cases = json.loads((SPEC / 'fixtures/hub/cases.json').read_text())
        cls.patch = json.loads((SPEC / 'fixtures/hub/patch.json').read_text())

    def validate(self, rule, value, valid=True):
        root = dict(self.root, **{'$ref': f'#/$defs/{rule}'})
        self.assertEqual(Draft202012Validator(root, registry=self.registry).is_valid(value), valid)
        validator = Schema(f'fixture = {rule}\n' + self.cddl)
        if valid:
            validator.validate_cbor(encode(value))
        else:
            with self.assertRaises(ValidationError):
                validator.validate_cbor(encode(value))

    def test_hub_dual_schema_and_golden(self):
        Draft202012Validator.check_schema(self.root)
        self.assertEqual(len(self.cases), len({c['name'] for c in self.cases}))
        covered = set()
        for case in self.cases:
            with self.subTest(case=case['name']):
                self.validate(case['type'], case['value'], case['valid'])
                if case['valid']:
                    covered.add(case['type'])
                    raw = bytes.fromhex(case['cbor_hex'])
                    self.assertEqual(encode(case['value']), raw)
                    self.assertEqual(read_frame(struct.pack('>I', len(raw)) + raw), case['value'])
        self.assertEqual(covered, set(self.root['$defs']))
        print(f'hub: {len(self.cases)} dual-schema cases; {len(covered)} golden rule vectors')

    def test_standard_occurrence_and_absolute_end_regressions(self):
        self.assertNotRegex(self.cddl, r'\*\s+\d')
        for bound in ['*0 tstr', '*17 hub-method', '*64 event', '*256 hub-health-item']:
            self.assertIn(bound, self.cddl)
        for rule, schema in self.root['$defs'].items():
            if 'pattern' in schema:
                self.assertNotIn('$', schema['pattern'])
                self.assertTrue(any(c['name'] == rule + '-final-lf-0' and not c['valid'] for c in self.cases))
                self.assertTrue(any(c['name'] == rule + '-end-control-0' and c['valid'] for c in self.cases))

    def test_frame_rejections(self):
        for case in json.loads((SPEC / 'fixtures/hub/framing.json').read_text()):
            with self.subTest(case=case['name']), self.assertRaises((ValueError, cbor2.CBORDecodeError, UnicodeError)):
                read_frame(bytes.fromhex(case['hex']))

    def test_observation_context_required(self):
        for method in ['manage.read', 'manage.tail_log', 'hub.status']:
            rule = method.replace('.', '-') + '-request'
            good = next(c['value'] for c in self.cases if c['name'] == rule)
            for context, valid in [(None, False), ({}, False), ({'ticket': '6' * 32}, False),
                                   ({'actor': good['context']['actor'], 'ticket': '6' * 32}, True)]:
                value = copy.deepcopy(good)
                if context is None:
                    del value['context']
                else:
                    value['context'] = context
                self.validate(rule, value, valid)

    def verify(self, raw, signature, signer):
        with tempfile.TemporaryDirectory() as td:
            p = Path(td)
            (p / 'key').write_bytes(bytes.fromhex('302a300506032b6570032100' + signer))
            (p / 'sig').write_bytes(bytes.fromhex(signature))
            (p / 'msg').write_bytes(raw)
            return subprocess.run(['openssl', 'pkeyutl', '-verify', '-pubin', '-keyform', 'DER', '-inkey', str(p / 'key'),
                '-rawin', '-in', str(p / 'msg'), '-sigfile', str(p / 'sig')], capture_output=True).returncode == 0

    def test_signing_vectors_and_target_binding(self):
        v = self.patch
        raw = signing(v['patch'], v['hub'], v['coglet'])
        self.assertEqual(raw.hex(), v['preimage_hex'])
        self.assertTrue(self.verify(raw, v['patch']['signature'], v['hub']))
        for bad in [raw + b'X', signing(v['patch'], v['hub'], 'coglet:' + '0' * 64),
                    signing(dict(v['patch'], generation=2), v['hub'], v['coglet'])]:
            self.assertFalse(self.verify(bad, v['patch']['signature'], v['hub']))
        for vector in json.loads((SPEC / 'fixtures/hub/signatures.json').read_text()):
            self.validate('hub-signed-delegation', {k: v for k, v in vector.items() if k != 'preimage_hex'})
            raw = b'coglet/hub/0:delegation\n' + encode(vector['statement'])
            self.assertEqual(raw.hex(), vector['preimage_hex'])
            self.assertTrue(self.verify(raw, vector['signature'], vector['signer']))

    def test_patch_install_and_boundary_candidate(self):
        v = self.patch
        self.validate('manage-patch', v['patch'])
        files, generation = apply({}, 0, v['patch'], installed=False)
        self.assertEqual((files, generation), (v['files'], 1))
        update = dict(v['patch'], base=tree(files), generation=2)
        self.assertEqual(apply(files, generation, update), (files, 2))
        # A higher generation rollback is legal, and an empty put is not deletion.
        update['files'] = [{'path': 'empty', 'bytes': ''}]
        update['revision'] = tree(dict(files, empty=''))
        new, gen = apply(files, generation, update)
        self.assertEqual(new['empty'], '')
        update = dict(update, base=tree(new), revision=tree(files), generation=3, files=[{'path': 'empty'}])
        self.assertEqual(apply(new, gen, update), (files, 3))

    def test_patch_stale_base_replay_bad_path_missing_executable(self):
        v = self.patch
        cases = json.loads((SPEC / 'fixtures/hub/patch-rejections.json').read_text())
        self.assertEqual({c['name'] for c in cases}, {'stale-base', 'equal-generation', 'lower-generation', 'bad-path', 'missing-executable'})
        for c in cases:
            with self.subTest(case=c['name']):
                self.assertTrue(self.verify(signing(c['patch'], v['hub'], v['coglet']), c['patch']['signature'], v['hub']))
                before = copy.deepcopy(v['files'])
                with self.assertRaises(Rejected) as e:
                    apply(v['files'], 1, c['patch'])
                self.assertEqual((e.exception.code, e.exception.reason), (c['code'], c['reason']))
                self.assertEqual(v['files'], before)
        for path in ['/code/a', '../a', './a', 'a/../b', 'a//b', 'a/', 'a\\b', 'a\x00b', 'a\nb', 'a/./b']:
            self.assertFalse(code_path(path), path)

    def test_patch_oversize_boundary(self):
        size_check(MAX_PATCH)
        for size in [MAX_PATCH + 1, 2**64 - 1]:
            with self.assertRaises(Rejected) as e:
                size_check(size)
            self.assertEqual(e.exception.code, 'resource_exhausted')
        # Actual encoded-byte overrun, not a declared-size-only oracle.
        patch = dict(self.patch['patch'], files=[{'path': 'large', 'bytes': '00' * (MAX_PATCH // 2)}])
        with self.assertRaisesRegex(Rejected, 'patch_size'):
            apply({}, 0, patch, installed=False)

    def test_log_paths_cover_abi_and_reject_noncanonical_names(self):
        from manage_oracle import log_bytes
        good = ['/logs/café', '/logs/a b', '/logs/e\u0301', '/logs/😀/日誌',
                '/logs/' + 'é' * 600, '/logs/.hidden', '/logs/...', '/logs/a.b',
                '/logs/quote"name', '/logs/\u0085']
        bad = ['/logs', '/logs/', '/data/a', 'logs/a', '/logs//a', '/logs/a/',
               '/logs/.', '/logs/..', '/logs/a/../b', '/logs/a/./b', '/logs/a\\b']
        bad += ['/logs/a' + chr(c) + 'b' for c in [*range(32), 127]]
        for path in good + bad:
            valid = path in good
            with self.subTest(path=path):
                self.validate('manage-log-path', path, valid)
                self.validate('manage-log-entry', {'path': path, 'file_offset': 0, 'hex': 'ff'}, valid)
                if valid:
                    self.assertEqual(json.loads(log_bytes(path, 0, b'\xff'))['path'], path)
                else:
                    with self.assertRaises(Rejected):
                        log_bytes(path, 0, b'\xff')

    def test_unicode_log_jsonl_and_split_cursor_restart(self):
        from manage_oracle import log_bytes
        first = log_bytes('/logs/café', 0, b'\x00\xff')
        second = log_bytes('/logs/a b', 0, b'next')
        self.assertEqual(first, '{"file_offset":0,"hex":"00ff","path":"/logs/café"}\n'.encode())
        stream = first + second
        # Byte cursor inside a multibyte code point, not a character/entry cursor.
        split = first.index('é'.encode()) + 1
        prefix, offset = tail(stream, 0, split)
        with tempfile.TemporaryDirectory() as td:
            p = Path(td) / 'log'
            p.write_bytes(stream)
            suffix = subprocess.check_output([__import__('sys').executable, '-c',
                'import pathlib,sys; sys.stdout.buffer.write(pathlib.Path(sys.argv[1]).read_bytes()[int(sys.argv[2]):])', str(p), str(offset)])
        self.assertEqual(prefix + suffix, stream)
        self.assertEqual(tail(stream, offset), (suffix, len(stream)))
        self.assertEqual([json.loads(line)['path'] for line in (prefix + suffix).splitlines()],
                         ['/logs/café', '/logs/a b'])

    def test_tail_across_fresh_process_restart(self):
        v = json.loads((SPEC / 'fixtures/hub/tail-restart.json').read_text())
        before, after = bytes.fromhex(v['before_hex']), bytes.fromhex(v['after_hex'])
        first, offset = tail(before, 0)
        self.assertEqual(offset, v['restart_offset'])
        with tempfile.TemporaryDirectory() as td:
            p = Path(td) / 'log'
            p.write_bytes(before + after)
            # A new interpreter receives only persisted bytes and durable cursor.
            raw = subprocess.check_output([__import__('sys').executable, '-c',
                'import pathlib,sys; sys.stdout.buffer.write(pathlib.Path(sys.argv[1]).read_bytes()[int(sys.argv[2]):])', str(p), str(offset)])
        self.assertEqual(first + raw, before + after)
        self.assertEqual(tail(before + after, offset), (after, len(before + after)))
        self.assertEqual(tail(before, len(before)), (b'', len(before)))
        with self.assertRaises(Rejected):
            tail(before, len(before) + 1)

    def test_alias_window_and_canonical_schema(self):
        for name in ['read', 'write', 'pause', 'step']:
            old = 'de' + 'bug.' + name
            self.assertEqual(normalize(old, '2027-02-24T23:59:59Z'), 'manage.' + name)
            self.assertEqual(normalize(old, '2027-02-25T00:00:00Z'), old)
            rule = 'manage-' + name + '-request'
            request = copy.deepcopy(next(c['value'] for c in self.cases if c['name'] == rule))
            request['method'] = old
            self.validate('hub-request', request, False)
            request['method'] = normalize(old, '2026-11-25T00:00:00Z')
            self.validate('hub-request', request)
        self.assertFalse(any(m.startswith('hub.' + 'git') for m in self.root['$defs']['hub-method']['enum']))

    def test_build_provenance_binding(self):
        v = self.patch
        record = decode(bytes.fromhex(v['files']['build/provenance.cbor']))
        self.validate('hub-build-record', record)
        self.assertEqual(record['source_tree'], tree(v['source_files']))
        for name, value in v['build_inputs'].items():
            self.assertEqual(record[name + '_sha256'], hashlib.sha256(bytes.fromhex(value)).hexdigest())
        output = bytes.fromhex(v['files']['bin/step.wasm'])
        self.assertEqual(record['rewritten_sha256'], hashlib.sha256(output).hexdigest())
        self.assertNotEqual(record['rewritten_sha256'], hashlib.sha256(output + b'changed').hexdigest())
        self.assertEqual(record['original_sha256'], hashlib.sha256(bytes.fromhex(v['original_hex'])).hexdigest())


if __name__ == '__main__':
    unittest.main()
