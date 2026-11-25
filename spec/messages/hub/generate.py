"""Generate hub shapes and diagnostic fixtures; schemas remain normative.

Run with the spec fixture dependencies installed. Does not implement a hub.
"""
import copy
import json
from pathlib import Path
import sys

SPEC = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(SPEC / 'tests'))
from test_common import encode

COMMON = json.loads((SPEC / 'messages/common.schema.json').read_text())
D = {}
C = []
EX = {}
N = 'a' * 64
H = 'b' * 64
ID = 'c' * 32
COG = 'coglet:' + N
HASH = 'e' * 64


def add(name, cddl, schema, example):
    C.append(f'{name} = {cddl}')
    D[name] = schema
    EX[name] = example
    return name


def ref(name):
    if name in COMMON['$defs']:
        return {'$ref': f'common.schema.json#/$defs/{name}'}
    return {'$ref': f'#/$defs/{name}'}


def obj(name, fields, optional=()):
    c = ', '.join(('? ' if k in optional else '') + f'{json.dumps(k)}: {t}' for k, t in fields.items())
    if not fields:
        c = '*0 tstr => profile-value'
    add(name, '{' + c + '}', {'type': 'object', 'properties': {k: ref(t) for k, t in fields.items()},
        'required': [k for k in fields if k not in optional], 'additionalProperties': False},
        {k: copy.deepcopy(EX[t]) for k, t in fields.items() if k not in optional})
    return name


def enum(name, values):
    return add(name, ' / '.join(json.dumps(v) for v in values), {'enum': values}, values[0])


def union(name, names):
    return add(name, ' / '.join(names), {'anyOf': [ref(n) for n in names]}, copy.deepcopy(EX[names[0]]))


def array(name, t, maximum):
    return add(name, f'[*{maximum} {t}]', {'type': 'array', 'items': ref(t), 'maxItems': maximum}, [copy.deepcopy(EX[t])])


def text(name, regex, sample):
    # JSON Schema searches rather than full-matches; $ accepts a final LF.
    # Every $ here is an end assertion (none is escaped/in a character class).
    # Replace ALL alternatives' ends with the portable absolute-end assertion.
    pattern = regex.replace('$', r'(?![\s\S])')
    return add(name, 'tstr .regexp ' + json.dumps(regex), {'type': 'string', 'pattern': pattern}, sample)


def bounded(name, low, high):
    return add(name, f'{low}..{high}', {'type': 'integer', 'minimum': low, 'maximum': high}, low)


# Examples for imported definitions only; no copied schema definitions.
EX.update({'node-id': N, 'coglet-id': COG, 'hub-id': 'hub:' + H,
           'source-id': 'hub:' + H, 'correlation-id': ID, 'action-id': ID,
           'uint64': 0, 'profile-map': {}, 'method-name': 'manage.read',
           'capability-name': 'hub.note', 'version-offer': {'versions': ['1.0']},
           'version-selection': {'version': '1.0'}, 'error': {'code': 'conflict', 'message': 'Conflict', 'retryable': False},
           'event': {'source': COG, 'kind': 'hub.note', 'seq': 0, 'logical_time': 0, 'payload': {}, 'correlation_id': ID},
           'action': {'id': ID, 'coglet': COG, 'capability': 'test.move', 'args': {}, 'deadline': 100, 'correlation_id': ID},
           'action-accept': {'id': ID, 'correlation_id': ID, 'accepted': True}})
add('hub-null', 'null', {'type': 'null'}, None)
add('hub-bool', 'bool', {'type': 'boolean'}, True)
text('hub-hash', '^[0-9a-f]{64}$', HASH)
enum('hub-build-version', ['1.0'])
obj('hub-build-record', {'version': 'hub-build-version', 'source_tree': 'hub-hash',
    'recipe_sha256': 'hub-hash', 'policy_sha256': 'hub-hash', 'transform_sha256': 'hub-hash',
    'original_sha256': 'hub-hash', 'rewritten_sha256': 'hub-hash'})
text('hub-signature', '^[0-9a-f]{128}$', 'f' * 128)
text('hub-secret', '^[0-9a-f]{64}$', '1' * 64)
add('hub-text', 'tstr .size (0..65536)', {'type': 'string', 'maxLength': 65536}, 'hello')
text('hub-path', '^(/data/|/logs/)[a-zA-Z0-9_./-]{1,512}$|^@step/[0-9]{1,20}$', '/data/state.json')
text('hub-write-path', '^/data/[a-zA-Z0-9_./-]{1,512}$', '/data/state.json')
text('hub-chunk-hex', '^([0-9a-f]{2}){1,32768}$', '00')
bounded('hub-timeout', 1, 300000)
bounded('hub-size', 1, 67108864)
bounded('hub-read-size', 1, 65536)
bounded('hub-queue', 0, 256)
union('hub-maybe-tree', ['hub-hash', 'hub-null'])
union('hub-maybe-generation', ['uint64', 'hub-null'])
union('hub-target', ['coglet-id', 'hub-null'])
union('hub-maybe-step', ['uint64', 'hub-null'])
union('hub-maybe-error', ['error', 'hub-null'])
union('hub-maybe-secret', ['hub-secret', 'hub-null'])
obj('hub-empty', {})
enum('hub-state', ['stopped', 'running', 'paused', 'faulted'])
methods = ['hub.enroll', 'hub.install', 'hub.start', 'hub.stop', 'hub.status',
           'manage.patch', 'manage.tail_log', 'hub.events', 'hub.actions',
           'manage.read', 'manage.write', 'manage.pause', 'manage.step',
           'hub.inbox', 'hub.health', 'hub.receipt', 'hub.cancel']
enum('hub-method', methods)
array('hub-methods', 'hub-method', len(methods))
enum('hub-purpose-delegation', ['delegation'])
obj('hub-delegation-statement', {'purpose': 'hub-purpose-delegation', 'coglet': 'coglet-id', 'device': 'node-id',
    'hub': 'node-id', 'generation': 'uint64', 'expires': 'uint64', 'methods': 'hub-methods'})
obj('hub-signed-delegation', {'statement': 'hub-delegation-statement', 'signer': 'node-id', 'signature': 'hub-signature'})
# bytes remains a diagnostic/profile hex string, never CBOR major 2.
text('manage-file-bytes', '^([0-9a-f]{2})*$', '')
# Segment length/count are semantic bounds: nesting bounded repetitions exceeds
# Go RE2's repeat-product limit. The portable shape rejects separators/controls.
text('manage-code-path', '^[a-zA-Z0-9_.-]+(/[a-zA-Z0-9_.-]+)*$', 'bin/step.wasm')
obj('manage-file-change', {'path': 'manage-code-path', 'bytes': 'manage-file-bytes'}, optional=['bytes'])
EX['manage-file-change']['bytes'] = '0061736d01000000'
array('manage-files', 'manage-file-change', 4096)
obj('manage-patch', {'base': 'hub-hash', 'revision': 'hub-hash', 'generation': 'uint64',
                     'files': 'manage-files', 'signature': 'hub-signature'}, optional=['base'])
obj('manage-patch-signing', {'hub': 'node-id', 'coglet': 'coglet-id', 'base': 'hub-hash',
    'revision': 'hub-hash', 'generation': 'uint64', 'files': 'manage-files'}, optional=['base'])
obj('hub-status', {'state': 'hub-state', 'live_tree': 'hub-maybe-tree', 'generation': 'hub-maybe-generation',
                  'step': 'uint64', 'pending': 'hub-queue', 'error': 'hub-maybe-error'})
obj('hub-receipt-queued', {'operation_id': 'correlation-id', 'state': enum('hub-queued', ['queued']), 'step': 'hub-null', 'error': 'hub-null'})
obj('hub-receipt-applied', {'operation_id': 'correlation-id', 'state': enum('hub-applied', ['applied']), 'step': 'uint64', 'error': 'hub-null'})
obj('hub-receipt-cancelled', {'operation_id': 'correlation-id', 'state': enum('hub-cancelled', ['cancelled']), 'step': 'hub-null', 'error': 'hub-null'})
obj('hub-receipt-failed', {'operation_id': 'correlation-id', 'state': enum('hub-failed', ['failed']), 'step': 'hub-null', 'error': 'error'})
union('hub-mutation-receipt', ['hub-receipt-' + s for s in ['queued', 'applied', 'cancelled', 'failed']])
obj('hub-inbox-delivery', {'id': 'correlation-id', 'sender': 'source-id', 'event': 'event'})
array('hub-event-batch', 'event', 64)
obj('hub-health-item', {'coglet': 'coglet-id', 'status': 'hub-status'})
array('hub-health-items', 'hub-health-item', 256)
obj('hub-enroll-params', {'offer': 'version-offer', 'invite': 'hub-maybe-secret'})
obj('hub-install-params', {'delegation': 'hub-signed-delegation', 'initial': 'hub-hash'})
obj('manage-patch-params', {'size': 'hub-size', 'sha256': 'hub-hash'})
obj('manage-tail-log-params', {'from_offset': 'uint64'})
obj('hub-events-params', {'offset': 'uint64', 'events': 'hub-event-batch'})
obj('hub-actions-params', {'action': 'action'})
obj('hub-read-params', {'path': 'hub-path', 'offset': 'uint64', 'limit': 'hub-read-size'})
obj('hub-write-params', {'path': 'hub-write-path', 'text': 'hub-text', 'expected_hash': 'hub-hash',
                         'actor': 'source-id', 'ticket': 'correlation-id'}, optional=['ticket'])
obj('hub-inbox-params', {'message': 'hub-inbox-delivery', 'actor': 'source-id', 'ticket': 'correlation-id'}, optional=['ticket'])
obj('hub-health-params', {'device': 'node-id', 'uptime_ms': 'uint64', 'queue_depth': 'uint64',
                          'available_bytes': 'uint64', 'coglets': 'hub-health-items'})
obj('hub-operation-params', {'operation_id': 'correlation-id'})
params = ['hub-enroll-params', 'hub-install-params', 'hub-empty', 'hub-empty', 'hub-empty',
          'manage-patch-params', 'manage-tail-log-params', 'hub-events-params', 'hub-actions-params',
          'hub-read-params', 'hub-write-params', 'hub-empty', 'hub-empty', 'hub-inbox-params',
          'hub-health-params', 'hub-operation-params', 'hub-operation-params']
requests = []
obj('hub-context', {'actor': 'source-id', 'ticket': 'correlation-id'}, optional=['ticket'])
for method, p in zip(methods, params):
    name = method.replace('.', '-') + '-request'
    m = enum(name + '-method', [method])
    target = 'hub-null' if method in ['hub.enroll', 'hub.health'] else 'coglet-id'
    fields = {'method': m, 'target': target, 'correlation_id': 'correlation-id',
              'timeout_ms': 'hub-timeout', 'params': p}
    if method in ['hub.install', 'hub.start', 'hub.stop', 'hub.status', 'manage.patch', 'manage.tail_log', 'manage.read', 'hub.actions', 'manage.pause', 'manage.step', 'hub.receipt', 'hub.cancel']:
        fields['context'] = 'hub-context'
    obj(name, fields)
    requests.append(name)
union('hub-request', requests)
obj('hub-enroll-result', {'selection': 'version-selection', 'device': 'node-id', 'hub': 'node-id'})
obj('hub-read-result', {'step': 'uint64', 'snapshot_hash': 'hub-hash', 'text': 'hub-text',
                       'next_offset': 'uint64', 'eof': 'hub-bool'})
obj('hub-events-result', {'next_offset': 'uint64'})
obj('hub-step-result', {'receipt': 'hub-mutation-receipt', 'step': 'uint64'})
results = ['hub-enroll-result', 'hub-mutation-receipt', 'hub-status', 'action-accept',
           'hub-read-result', 'hub-events-result', 'hub-step-result', 'hub-empty']
union('hub-result', results)
obj('hub-success', {'type': enum('hub-success-type', ['reply']), 'correlation_id': 'correlation-id', 'result': 'hub-result'})
obj('hub-failure', {'type': enum('hub-failure-type', ['error']), 'correlation_id': 'correlation-id', 'error': 'error'})
obj('hub-credit', {'type': enum('hub-credit-type', ['credit']), 'correlation_id': 'correlation-id', 'bytes': 'uint64'})
obj('hub-chunk', {'type': enum('hub-chunk-type', ['chunk']), 'correlation_id': 'correlation-id', 'offset': 'uint64', 'hex': 'hub-chunk-hex'})
obj('hub-end', {'type': enum('hub-end-type', ['end']), 'correlation_id': 'correlation-id'})
# Log entries describe ABI paths, NOT the narrower management request grammar.
# Portable (including RE2) component alternatives exclude exactly . and .. .
log_char = r'[^/\x00-\x1f\x7f\\]'
log_non_dot = r'[^/.\x00-\x1f\x7f\\]'
log_component = f'({log_non_dot}{log_char}*|[.]{log_non_dot}{log_char}*|[.][.]{log_char}+)'
text('manage-log-path', f'^/logs/{log_component}(/{log_component})*$', '/logs/stdout')
# JSON Schema uses ECMA Unicode escapes; CDDL/generated RE2 uses hex escapes.
D['manage-log-path']['pattern'] = D['manage-log-path']['pattern'].replace(r'\x', r'\u00')
obj('manage-log-entry', {'path': 'manage-log-path', 'file_offset': 'uint64', 'hex': 'hub-chunk-hex'})
obj('manage-tail-log-result', {'next_offset': 'uint64'})
obj('manage-log', {'type': enum('manage-log-type', ['log']), 'correlation_id': 'correlation-id',
                   'offset': 'uint64', 'hex': 'hub-chunk-hex'})
frames = ['hub-request', 'hub-success', 'hub-failure', 'hub-credit', 'hub-chunk', 'hub-end', 'manage-log']
union('hub-frame', frames)
# Use a literal root rule first so concatenating common.cddl cannot change it.
(SPEC / 'messages/hub.cddl').write_text('; Generated by hub/generate.py; import common.cddl unchanged.\nwire = hub-frame\n' + '\n'.join(C) + '\n')
root = {'$schema': 'https://json-schema.org/draft/2020-12/schema',
        '$id': 'https://coglet.dev/spec/messages/hub.schema.json', '$ref': '#/$defs/hub-frame', '$defs': D}
(SPEC / 'messages/hub.schema.json').write_text(json.dumps(root, indent=2) + '\n')
cases = []
# Every local definition has a golden; every closed record has field negatives.
for name, value in EX.items():
    if name not in D:
        continue
    cases.append({'name': name, 'type': name, 'valid': True, 'value': value, 'cbor_hex': encode(value).hex()})
    if D[name].get('type') == 'object':
        bad = dict(copy.deepcopy(value), unknown=True)
        cases.append({'name': name + '-unknown', 'type': name, 'valid': False, 'value': bad})
        for field in D[name]['required']:
            bad = copy.deepcopy(value)
            del bad[field]
            cases.append({'name': name + '-missing-' + field, 'type': name, 'valid': False, 'value': bad})
for name, value in [('hub-timeout', 0), ('hub-timeout', 300001), ('hub-chunk-hex', 'a'),
                    ('hub-signature', 'x' * 128), ('hub-write-path', '/code/main'),
                    ('hub-receipt-applied', {'operation_id': ID, 'state': 'applied', 'step': None, 'error': None}),
                    ('hub-event-batch', [EX['event']] * 65)]:
    cases.append({'name': f'negative-{len(cases)}', 'type': name, 'valid': False, 'value': value})
# Whole-string grammar regressions, including both path alternatives, selected
# rules, method-specific requests, and the enclosing public frame union.
def fixture(name, rule, value, valid):
    case = {'name': name, 'type': rule, 'valid': valid, 'value': value}
    if valid:
        case['cbor_hex'] = encode(value).hex()
    cases.append(case)

for rule, schema in D.items():
    if 'pattern' in schema:
        samples = [EX[rule]]
        if rule == 'hub-path':
            samples += ['/logs/run.jsonl', '@step/42']
        for i, sample in enumerate(samples):
            fixture(f'{rule}-end-control-{i}', rule, sample, True)
            fixture(f'{rule}-final-lf-{i}', rule, sample + '\n', False)

nested_patterns = [
    ('hub-hash', 'manage-patch-request', ('params', 'sha256')),
    ('hub-signature', 'hub-install-request', ('params', 'delegation', 'signature')),
    ('hub-secret', 'hub-enroll-request', ('params', 'invite')),
    ('hub-path', 'manage-read-request', ('params', 'path')),
    ('hub-write-path', 'manage-write-request', ('params', 'path')),
    ('hub-chunk-hex', 'hub-chunk', ('hex',)),
]
for pattern_rule, request_rule, path in nested_patterns:
    samples = [EX[pattern_rule]]
    if pattern_rule == 'hub-path':
        samples += ['/logs/run.jsonl', '@step/42']
    for i, sample in enumerate(samples):
        for valid in [True, False]:
            value = copy.deepcopy(EX[request_rule])
            parent = value
            for key in path[:-1]:
                parent = parent[key]
            parent[path[-1]] = sample if valid else sample + '\n'
            roots = [request_rule, 'hub-frame']
            if request_rule in requests:
                roots.append('hub-request')
            for root_rule in roots:
                fixture(f'{pattern_rule}-nested-{i}-{valid}-{root_rule}', root_rule, value, valid)

# These golden paths run through every generated SDK as well as both references.
for i, path in enumerate(['/logs/café', '/logs/a b', '/logs/e\u0301', '/logs/😀/日誌',
                          '/logs/' + 'é' * 600, '/logs/.hidden', '/logs/...', '/logs/quote"name', '/logs/\u0085']):
    fixture(f'log-path-utf8-{i}', 'manage-log-path', path, True)
    fixture(f'log-entry-utf8-{i}', 'manage-log-entry',
            {'path': path, 'file_offset': 0, 'hex': 'ff'}, True)
for i, path in enumerate(['/logs', '/logs/', '/data/a', 'logs/a', '/logs//a', '/logs/a/',
                          '/logs/.', '/logs/..', '/logs/a/../b', '/logs/a/./b', '/logs/a\\b']
                         + ['/logs/a' + chr(c) + 'b' for c in [*range(32), 127]]):
    fixture(f'log-path-invalid-{i}', 'manage-log-path', path, False)
    fixture(f'log-entry-invalid-{i}', 'manage-log-entry',
            {'path': path, 'file_offset': 0, 'hex': 'ff'}, False)

folder = SPEC / 'fixtures/hub'
folder.mkdir(exist_ok=True)
(folder / 'cases.json').write_text(json.dumps(cases, indent=2) + '\n')
print(f'Generated {len(D)} rules and {len(cases)} fixtures')
