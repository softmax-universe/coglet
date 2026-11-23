"""ABI format validation; deliberately not engine/harness conformance."""
import copy
import hashlib
import json
from pathlib import PurePosixPath
import unittest

import cbor2
from jsonschema import Draft202012Validator
from pycddl import Schema, ValidationError
from referencing import Registry, Resource

from test_common import SPEC, decode, encode

BASE = 'https://coglet.dev/spec/'


def digest(value):
    return hashlib.sha256(encode(value)).hexdigest()


def record_hash(rec):
    return digest({k: v for k, v in rec.items() if k != 'record_hash'})


def valid_path(path):
    parts = path.split('/')
    return (len(parts) >= 3 and parts[0] == '' and parts[1] in ('code', 'data', 'logs')
            and all(p not in ('', '.', '..') and '\0' not in p for p in parts[1:])
            and str(PurePosixPath(path)) == path)


def check_manifest(man):
    for key in ('subscriptions', 'required_capabilities', 'exclusive_outputs'):
        if len(set(man[key])) != len(man[key]):
            raise ValueError('duplicate capability')
    if not set(man['exclusive_outputs']) <= set(man['required_capabilities']):
        raise ValueError('exclusive output not required')


def check_record(rec):
    """Cross-field invariants independent of either shape validator."""
    if record_hash(rec) != rec['record_hash']:
        raise ValueError('record hash')
    for side in ('input', 'output'):
        files = rec[side + '_files']
        if not all(valid_path(p) for p in files):
            raise ValueError('unsafe path')
        if digest(files) != rec[side + '_hash']:
            raise ValueError('tree hash')
    # Undo the exact boundary interventions to reconstruct the preceding tree.
    before = dict(rec['input_files'])
    for change in reversed(rec['boundary']):
        if not valid_path(change['path']) or change['path'].startswith('/code/'):
            raise ValueError('boundary mount')
        if before.get(change['path']) != change['after']:
            raise ValueError('boundary after bytes')
        if change['before'] is None:
            before.pop(change['path'], None)
        else:
            before[change['path']] = change['before']
    if digest(before) != rec['before_hash']:
        raise ValueError('boundary before hash')
    if rec['status'] == 'aborted' and rec['output_files'] != rec['input_files']:
        raise ValueError('aborted writes')
    for op, key in [('emit', 'actions'), ('timer', 'timers'), ('llm', 'llm_requests'), ('log', 'logs')]:
        expected = [c['argument'] for c in rec['calls'] if c['op'] == op and c['result'] == 0] if rec['status'] == 'committed' else []
        if rec[key] != expected:
            raise ValueError('effect list does not match calls')
    for action in rec['actions']:
        if action['coglet'] != rec['coglet'] or action['capability'] not in rec['grants']:
            raise ValueError('ungranted action')
    for request in rec['llm_requests']:
        if request['capability'] not in rec['grants']:
            raise ValueError('ungranted LLM')
    # Enforce mount writes and append-only files independently of shape.
    for path, value in rec['input_files'].items():
        after = rec['output_files'].get(path)
        if (path.startswith('/code/') or path == '/data/inbox.jsonl') and after != value:
            raise ValueError('read-only write')
        if path.startswith('/logs/') and (after is None or not after.startswith(value)):
            raise ValueError('log rewrite')
    if any(p.startswith('/code/') or p == '/data/inbox.jsonl' for p in rec['output_files'].keys() - rec['input_files'].keys()):
        raise ValueError('read-only create')
    for side in ('input', 'output'):
        raw = bytes.fromhex(rec[side + '_files'].get('/data/inbox.jsonl', ''))
        if raw and not raw.endswith(b'\n'):
            raise ValueError('partial inbox line')
        ids = set()
        for index, line in enumerate(raw.splitlines()):
            def pairs(items):
                out = dict(items)
                if len(out) != len(items):
                    raise ValueError('duplicate JSON key')
                return out
            message = json.loads(line, object_pairs_hook=pairs)
            encode(message)  # integer-only profile, including nested maps
            if message['index'] != index or message['sender'] != message['event']['source'] or message['id'] in ids:
                raise ValueError('inbox sequence, sender or duplicate')
            if message['event']['kind'] == 'llm.reply':
                reply = message['event']['payload']
                if reply['status'] not in ('succeeded', 'failed'):
                    raise ValueError('reply status')
            ids.add(message['id'])
    if len(rec['events']) > rec['budgets']['max_events']:
        raise ValueError('batch cap')
    if rec['status'] == 'aborted':
        kind = rec['failure']['kind']
        code = rec['failure']['error']['code']
        if kind == 'wall' and code != 'deadline_exceeded':
            raise ValueError('wall failure code')
        if kind in ('compute', 'memory', 'host_bytes') and code != 'resource_exhausted':
            raise ValueError('resource failure code')
    # Reserved log file contains exactly the host-generated lines, not arbitrary
    # guest bytes. Raw stdout/stderr use separate append-only files.
    expected_logs = bytes.fromhex(rec['input_files'].get('/logs/events.jsonl', ''))
    expected_logs += b''.join((json.dumps(log, sort_keys=True, separators=(',', ':'), ensure_ascii=False)+'\n').encode() for log in rec['logs'])
    if bytes.fromhex(rec['output_files'].get('/logs/events.jsonl', '')) != expected_logs:
        raise ValueError('log effect bytes')
    old_inbox = bytes.fromhex(before.get('/data/inbox.jsonl', ''))
    new_inbox = bytes.fromhex(rec['input_files'].get('/data/inbox.jsonl', ''))
    if not new_inbox.startswith(old_inbox):
        raise ValueError('boundary inbox rewrite')
    for change in rec['boundary']:
        if change['target'] != rec['coglet']:
            raise ValueError('boundary target')
        if change['path'] == '/data/inbox.jsonl' and change['method'] != 'inbox.append':
            raise ValueError('inbox method')
    return before


def check_trace(trace, manifest_validator=None):
    previous = None
    files = None
    ids = set()
    requests = {}
    replies = set()
    for index, rec in enumerate(trace['records']):
        before = check_record(rec)
        if rec['step'] != index or rec['previous'] != previous or rec['trace_id'] != trace['trace_id'] or rec['coglet'] != trace['coglet']:
            raise ValueError('trace chain identity')
        if index == 0 and digest(before) != trace['initial_hash']:
            raise ValueError('initial hash')
        if files is not None and before != files:
            raise ValueError('snapshot chain')
        if index and rec['logical_time'] < trace['records'][index-1]['logical_time']:
            raise ValueError('time regressed')
        for key in ('code_hash', 'manifest_hash'):
            raw = bytes.fromhex(trace['blobs'][rec[key]])
            if hashlib.sha256(raw).hexdigest() != rec[key]:
                raise ValueError('blob mismatch')
        if rec['input_files'].get('/code/bin/step.wasm') != trace['blobs'][rec['code_hash']]:
            raise ValueError('code not pinned')
        man = decode(bytes.fromhex(trace['blobs'][rec['manifest_hash']]))
        check_manifest(man)
        if manifest_validator is not None:
            manifest_validator.validate(man)
        if man['id'] != rec['coglet']:
            raise ValueError('manifest identity')
        if rec['budgets']['compute_mode'] != man['budgets']['compute_mode']:
            raise ValueError('compute mode changed')
        if any(rec['budgets'][k] > man['budgets'][k] for k in man['budgets'] if k != 'compute_mode'):
            raise ValueError('budget exceeds manifest')
        if any(e['kind'] not in man['subscriptions'] and e['kind'] not in ('runtime.inbox', 'runtime.timer') for e in rec['events']):
            raise ValueError('unsubscribed event')
        if index and any(rec[k] != trace['records'][0][k] for k in ('code_hash', 'manifest_hash')):
            raise ValueError('trace code/manifest changed')
        for key in ('actions', 'timers', 'llm_requests'):
            for value in rec[key]:
                identity = (key, value['id'])
                if identity in ids:
                    raise ValueError('duplicate effect id')
                ids.add(identity)
        old_lines = len(bytes.fromhex(before.get('/data/inbox.jsonl', '')).splitlines())
        lines = bytes.fromhex(rec['input_files'].get('/data/inbox.jsonl', '')).splitlines()
        # All historical lines remain recorded; only newly appended replies may
        # consume a pending request. Pending requests span steps, never live calls.
        for line in lines[old_lines:]:
            message = json.loads(line)
            event = message['event']
            if event['kind'] == 'llm.reply':
                reply = event['payload']
                request = requests.get(reply['request_id'])
                if request is None or reply['request_id'] in replies or event['correlation_id'] != request['correlation_id']:
                    raise ValueError('uncorrelated or duplicate LLM reply')
                replies.add(reply['request_id'])
        for request in rec['llm_requests']:
            requests[request['id']] = request
        previous, files = rec['record_hash'], rec['output_files']
    for key, value in trace['blobs'].items():
        if hashlib.sha256(bytes.fromhex(value)).hexdigest() != key:
            raise ValueError('extra blob mismatch')


class AbiFixtures(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.abi = json.loads((SPEC / 'messages/abi.schema.json').read_text())
        cls.manifest = json.loads((SPEC / 'manifest.schema.json').read_text())
        common = json.loads((SPEC / 'messages/common.schema.json').read_text())
        cls.registry = Registry().with_resources((s['$id'], Resource.from_contents(s)) for s in [cls.abi, cls.manifest, common])
        for schema in (cls.abi, cls.manifest):
            Draft202012Validator.check_schema(schema)
        cls.cddl = (SPEC / 'messages/abi.cddl').read_text() + '\n' + (SPEC / 'messages/common.cddl').read_text()
        cls.cases = json.loads((SPEC / 'fixtures/abi/cases.json').read_text())

    def validator(self, rule):
        schema = self.manifest if rule == 'manifest' else dict(self.abi, **{'$ref': '#/$defs/' + rule})
        schema = dict(schema)
        schema.pop('anyOf', None)
        return Draft202012Validator(schema, registry=self.registry)

    def test_shapes_and_golden_cbor(self):
        covered = {}
        for case in self.cases:
            with self.subTest(case=case['name']):
                typ, value = case['type'], case['value']
                covered.setdefault(typ, set()).add(case['valid'])
                self.assertEqual(self.validator(typ).is_valid(value), case['valid'], 'JSON schema')
                try:
                    Schema('fixture = ' + typ + '\n' + self.cddl).validate_cbor(cbor2.dumps(value))
                    valid = True
                except ValidationError as exc:
                    valid = False
                    if case['valid']:
                        print(f"CDDL {case['name']}: {exc}")
                self.assertEqual(valid, case['valid'], 'CDDL')
                if valid:
                    self.assertEqual(encode(value).hex(), case['cbor_hex'])
                    self.assertEqual(decode(bytes.fromhex(case['cbor_hex'])), value)
        self.assertTrue(all(states == {True, False} for states in covered.values()))
        print(f'abi: {len(self.cases)} dual-schema fixtures')

    def test_semantic_fixtures(self):
        cases = json.loads((SPEC / 'fixtures/abi/semantic-cases.json').read_text())
        for case in cases:
            with self.subTest(case=case['name']):
                if case['valid']:
                    self.validator('step-record').validate(case['value'])
                    check_record(case['value'])
                else:
                    with self.assertRaises((ValueError, KeyError)):
                        check_record(case['value'])
        print(f'abi: {len(cases)} semantic fixtures')

    def test_trace_and_integrity_negative_controls(self):
        trace = next(c['value'] for c in self.cases if c['type'] == 'trace' and c['valid'])
        check_trace(trace)
        for field, value in [('previous', '0'*64), ('step', 1), ('manifest_hash', '0'*64), ('code_hash', '0'*64)]:
            bad = copy.deepcopy(trace)
            bad['records'][0][field] = value
            bad['records'][0]['record_hash'] = record_hash(bad['records'][0])
            with self.subTest(field=field), self.assertRaises((ValueError, KeyError)):
                check_trace(bad)
        bad = copy.deepcopy(trace)
        bad['initial_hash'] = '0'*64
        with self.assertRaises(ValueError):
            check_trace(bad)

    def test_async_reply_trace(self):
        trace = json.loads((SPEC / 'fixtures/abi/llm-reply-trace.json').read_text())
        self.validator('trace').validate(trace)
        check_trace(trace, self.validator('manifest'))
        # Correctly rehashed corruption still fails the semantic request binding.
        bad = copy.deepcopy(trace)
        rec = bad['records'][1]
        message = json.loads(bytes.fromhex(rec['input_files']['/data/inbox.jsonl']))
        self.validator('inbox-message').validate(message)
        self.validator('llm-reply').validate(message['event']['payload'])
        message['event']['payload']['request_id'] = '0'*32
        encoded = (json.dumps(message, sort_keys=True, separators=(',', ':'))+'\n').encode().hex()
        rec['input_files']['/data/inbox.jsonl'] = encoded
        rec['output_files']['/data/inbox.jsonl'] = encoded
        rec['boundary'][0]['after'] = encoded
        rec['input_hash'] = digest(rec['input_files'])
        rec['output_hash'] = digest(rec['output_files'])
        rec['record_hash'] = record_hash(rec)
        with self.assertRaisesRegex(ValueError, 'LLM reply'):
            check_trace(bad, self.validator('manifest'))

    def test_journal_frame_roundtrip_and_partial_tail(self):
        rec = next(c['value'] for c in self.cases if c['type'] == 'step-record' and c['valid'])
        raw = encode(rec)
        frame = len(raw).to_bytes(4, 'big') + raw
        self.assertEqual(int.from_bytes(frame[:4], 'big'), len(frame[4:]))
        self.assertEqual(decode(frame[4:]), rec)
        for truncated in (frame[:2], frame[:-1]):
            self.assertTrue(len(truncated) < 4 or int.from_bytes(truncated[:4], 'big') != len(truncated[4:]))

    def test_manifest_semantics(self):
        man = next(c['value'] for c in self.cases if c['type'] == 'manifest' and c['valid'])
        check_manifest(man)
        for field, value in [('exclusive_outputs', ['other.write']), ('subscriptions', ['sensor.tick']*2)]:
            bad = dict(man, **{field: value})
            with self.assertRaises(ValueError):
                check_manifest(bad)

    def test_data_only_harness_timelines(self):
        cases = json.loads((SPEC.parent / 'conformance/abi/cases.json').read_text())
        self.assertEqual(len(cases), len({c['name'] for c in cases}))
        for case in cases:
            with self.subTest(case=case['name']):
                self.assertEqual(len(case['steps']) % 3, 0)
                for start in range(0, len(case['steps']), 3):
                    send, expect, assertion = case['steps'][start:start+3]
                    self.assertEqual(send['send']['operation'], 'abi.step')
                    rec = expect['expect']['journal']
                    self.validator('step-record').validate(rec)
                    before = check_record(rec)
                    self.assertEqual(before, send['send']['initial_files'])
                    self.assertEqual(assertion['journal_hash'], rec['record_hash'])
                    self.assertEqual(expect['expect']['record_hash'], rec['record_hash'])
                    self.assertEqual(expect['expect']['state_hash'], rec['output_hash'])
        print(f'abi: {len(cases)} harness data timelines (not engine execution)')


if __name__ == '__main__':
    unittest.main()
