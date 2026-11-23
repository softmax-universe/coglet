"""Regenerate ABI schemas and golden data: python spec/tests/generate_abi.py.

Only fixtures/schemas are generated; this is not a runtime implementation.
"""
import copy
import hashlib
import json
from pathlib import Path
from test_common import encode

ROOT = Path(__file__).resolve().parents[2]
BASE = 'https://coglet.dev/spec/'
COMMON = BASE + 'messages/common.schema.json#/$defs/'
ABI = BASE + 'messages/abi.schema.json'
DEFS = {}
CDDL = []


def ref(name):
    return {'$ref': COMMON + name}


def local(name):
    return {'$ref': '#/$defs/' + name}


def obj(properties):
    return {'type': 'object', 'properties': properties,
            'required': list(properties), 'additionalProperties': False}


def arr(item, minimum=0):
    return {'type': 'array', 'items': item, 'minItems': minimum}


def rule(name, schema, cddl):
    DEFS[name] = schema
    CDDL.append(name + ' = ' + cddl)


def record(name, fields):
    rule(name, obj({k: s for k, s, c in fields}),
         '{' + ', '.join(json.dumps(k) + ': ' + c for k, s, c in fields) + '}')


def write(path, value):
    path = ROOT / path
    path.parent.mkdir(parents=True, exist_ok=True)
    # Upgrade all generated trace views together, including nested timelines.
    def complete(item):
        if isinstance(item, dict):
            if 'initial_hash' in item and isinstance(item.get('records'), list) and 'blobs' in item:
                item.setdefault('initial_actions', [])
                item.setdefault('trailing_boundary', None)
                if item['records']:
                    first = item['records'][0]
                    before = dict(first['input_files'])
                    for change in reversed(first['boundary']):
                        if change['before'] is None:
                            before.pop(change['path'], None)
                        else:
                            before[change['path']] = change['before']
                    item['initial_files'] = before
                    for key in ('code_hash', 'manifest_hash'):
                        item[key] = first[key]
                    item['final_hash'] = item['records'][-1]['output_hash']
            for child in item.values():
                complete(child)
        elif isinstance(item, list):
            for child in item:
                complete(child)
    complete(value)
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + '\n')


def digest(value):
    return hashlib.sha256(encode(value)).hexdigest()


U = ref('uint64')
S = {'type': 'string', 'minLength': 1}
HEX = {'type': 'string', 'pattern': '^([0-9a-f]{2})*(?![\\s\\S])'}
H = {'type': 'string', 'pattern': '^[0-9a-f]{64}(?![\\s\\S])'}
PATH_PATTERN = r'^/(code|data|logs)(/[^/\u0000-\u001f\u007f\\]+)+'
PATH = {'type': 'string', 'pattern': PATH_PATTERN + r'(?![\s\S])'}
rule('hash', H, 'tstr .regexp "^[0-9a-f]{64}$"')
rule('hex', HEX, 'tstr .regexp "^([0-9a-f]{2})*$"')
# CDDL regex consumers include Go RE2, which accepts \\xHH but not \\uHHHH.
# Keep JSON Schema's ECMA spelling and express the same ASCII range portably.
rule('path', PATH, 'tstr .regexp ' + json.dumps(PATH_PATTERN.replace(r'\u00', r'\x') + '$'))
# Path-key grammar and blob digest keys are cross-field semantic checks. pycddl
# 0.6.4 incorrectly applies a regex key control to the entire enclosing map.
rule('snapshot', {'type': 'object', 'additionalProperties': local('hex')}, '{* tstr => hex}')
positive = {'type': 'integer', 'minimum': 1, 'maximum': 2**64-1}
record('budgets', [('compute', positive, '1..18446744073709551615'),
                   ('compute_mode', S, 'tstr .size (1..18446744073709551615)'),
                   ('memory_pages', {'type': 'integer', 'minimum': 1, 'maximum': 65536}, '1..65536'),
                   ('wall_time_us', positive, '1..18446744073709551615'),
                   ('max_events', positive, '1..18446744073709551615'),
                   ('host_bytes', positive, '1..18446744073709551615')])
# Runtime-authored heard-state is embedded as deterministic CBOR in the reserved
# boundary file, not admitted as a standalone ABI root or guest event.
record('heard-span', [('end_ms', positive, '1..18446744073709551615'),
                      ('text', {'type': 'string'}, 'tstr')])
record('heard-item', [('surface', ref('surface-id'), 'surface-id'),
                      ('item', ref('correlation-id'), 'correlation-id'),
                      ('duration_ms', U, 'uint64'), ('played_ms', U, 'uint64'),
                      ('stopped', {'type': 'boolean'}, 'bool'),
                      ('spans', arr(local('heard-span')), '[* heard-span]'),
                      ('heard', {'type': 'string'}, 'tstr')])
record('heard-state', [('items', arr(local('heard-item')), '[* heard-item]')])
record('timer-request', [('id', ref('correlation-id'), 'correlation-id'), ('at', U, 'uint64')])
record('llm-request', [('id', ref('correlation-id'), 'correlation-id'),
                       ('correlation_id', ref('correlation-id'), 'correlation-id'),
                       ('capability', ref('capability-name'), 'capability-name'),
                       ('model', S, 'tstr .size (1..18446744073709551615)'),
                       ('input', ref('profile-map'), 'profile-map')])
record('log-entry', [('level', {'enum': ['debug', 'info', 'warn', 'error']}, '"debug" / "info" / "warn" / "error"'),
                     ('message', S, 'tstr .size (1..18446744073709551615)'),
                     ('fields', ref('profile-map'), 'profile-map')])
reply = []
for status, field, schema, cddl in [('succeeded', 'output', ref('profile-map'), 'profile-map'), ('failed', 'error', ref('error'), 'error')]:
    reply.append(obj({'request_id': ref('correlation-id'), 'status': {'const': status}, field: schema}))
rule('llm-reply', {'oneOf': reply}, '{request_id: correlation-id, status: "succeeded", output: profile-map} / {request_id: correlation-id, status: "failed", error: error}')
record('inbox-message', [('index', U, 'uint64'), ('id', ref('correlation-id'), 'correlation-id'), ('sender', ref('source-id'), 'source-id'), ('event', ref('event'), 'event')])
variants = []
texts = []
for op, name in [('timer', 'timer-request'), ('emit', 'action'), ('llm', 'llm-request'), ('log', 'log-entry')]:
    arg = ref(name) if name == 'action' else local(name)
    variants.append(obj({'op': {'const': op}, 'argument': arg, 'result': {'type': 'integer', 'minimum': 0, 'maximum': 4}}))
    texts.append('{op: "' + op + '", argument: ' + name + ', result: 0..4}')
variants.append(obj({'op': {'enum': ['timer', 'emit', 'llm', 'log']}, 'argument_hex': local('hex'), 'result': {'const': 1}}))
texts.append('{op: "timer" / "emit" / "llm" / "log", argument_hex: hex, result: 1}')
# Equivalent map group choices avoid pycddl's nested map-union backtracking bug.
rule('call', {'oneOf': variants}, '{ ' + ' // '.join('(' + text[1:-1] + ')' for text in texts) + ' }')
record('boundary-change', [('actor', ref('source-id'), 'source-id'), ('audit_id', ref('correlation-id'), 'correlation-id'),
                            ('ticket', {'anyOf': [ref('correlation-id'), {'type': 'null'}]}, 'correlation-id / null'),
                            ('method', ref('method-name'), 'method-name'), ('target', ref('coglet-id'), 'coglet-id'),
                            ('path', local('path'), 'path'), ('before', {'anyOf': [local('hex'), {'type': 'null'}]}, 'hex / null'),
                            ('after', {'anyOf': [local('hex'), {'type': 'null'}]}, 'hex / null')])
record('engine', [('build', S, 'tstr .size (1..18446744073709551615)'), ('policy', S, 'tstr .size (1..18446744073709551615)')])
record('usage', [(k, U, 'uint64') for k in ['compute', 'memory_pages', 'wall_time_us', 'host_bytes']])
record('failure', [('kind', {'enum': ['compute', 'memory', 'wall', 'host_bytes', 'trap', 'guest_return', 'invalid_argument']}, '"compute" / "memory" / "wall" / "host_bytes" / "trap" / "guest_return" / "invalid_argument"'), ('error', ref('error'), 'error')])
fields = [('version', {'const': '0.1'}, '"0.1"'), ('trace_id', ref('correlation-id'), 'correlation-id'), ('coglet', ref('coglet-id'), 'coglet-id')]
stepfields = fields + [(k, U, 'uint64') for k in ['step', 'logical_time', 'seed']] + [
 ('previous', {'anyOf': [local('hash'), {'type': 'null'}]}, 'hash / null')]
stepfields += [(k, local('hash'), 'hash') for k in ['code_hash', 'manifest_hash', 'before_hash', 'input_hash', 'output_hash', 'record_hash']]
stepfields += [('engine', local('engine'), 'engine'), ('budgets', local('budgets'), 'budgets'), ('grants', arr(ref('capability-name')), '[* capability-name]'), ('events', arr(ref('event'), 1), '[+ event]')]
stepfields += [(k, arr(ref(t) if t == 'action' else local(t)), '[* ' + t + ']') for k, t in [('boundary', 'boundary-change'), ('calls', 'call'), ('actions', 'action'), ('timers', 'timer-request'), ('llm_requests', 'llm-request'), ('logs', 'log-entry')]]
stepfields += [(k, local('snapshot'), 'snapshot') for k in ['input_files', 'output_files']] + [('usage', local('usage'), 'usage')]
props = {k: s for k, s, c in stepfields}
branches = []
for status in ['committed', 'aborted']:
    p = dict(props, status={'const': status})
    if status == 'aborted':
        p['failure'] = local('failure')
    branches.append(obj(p))
basecddl = ', '.join(k + ': ' + c for k, s, c in stepfields)
rule('step-record', {'oneOf': branches}, '{' + basecddl + ', status: "committed"} / {' + basecddl + ', status: "aborted", failure: failure}')
record('effect-ledger', [(k, arr(ref('correlation-id')), '[* correlation-id]') for k in ['actions', 'timers', 'llm_requests']])
record('boundary-record', fields + [
    ('logical_time', U, 'uint64'),
    ('previous', {'anyOf': [local('hash'), {'type': 'null'}]}, 'hash / null'),
    *[(k, local('hash'), 'hash') for k in ['before_hash', 'output_hash', 'record_hash']],
    ('boundary', arr(local('boundary-change'), 1), '[+ boundary-change]'),
    ('output_files', local('snapshot'), 'snapshot')])
# Import the common accept envelope and constrain it to terminal rejection.
rule('action-rejection', {'allOf': [ref('action-accept'), {'properties': {'accepted': {'const': False}}}]},
     'action-accept .and ({id: action-id, correlation_id: correlation-id, accepted: false, error: error})')
record('trace', fields + [('initial_effect_ids', local('effect-ledger'), 'effect-ledger'), ('initial_actions', arr(ref('action')), '[* action]'), ('initial_inbox_wakes', arr(U), '[* uint64]'), ('initial_hash', local('hash'), 'hash'), ('initial_files', local('snapshot'), 'snapshot'), ('code_hash', local('hash'), 'hash'), ('manifest_hash', local('hash'), 'hash'), ('final_hash', local('hash'), 'hash'), ('trailing_boundary', {'anyOf': [local('boundary-record'), {'type': 'null'}]}, 'boundary-record / null'), ('initial_timers', arr(local('timer-request')), '[* timer-request]'), ('initial_llm_requests', arr(local('llm-request')), '[* llm-request]'), ('records', arr(local('step-record')), '[* step-record]'), ('blobs', {'type': 'object', 'additionalProperties': local('hex')}, '{* tstr => hex}')])
manifest = obj({'version': {'const': '0.1'}, 'id': ref('coglet-id'),
                **{k: dict(arr(ref('capability-name')), uniqueItems=True) for k in ['subscriptions', 'required_capabilities', 'exclusive_outputs']},
                'budgets': {'$ref': ABI + '#/$defs/budgets'}})
manifest.update({'$schema': 'https://json-schema.org/draft/2020-12/schema', '$id': BASE + 'manifest.schema.json'})
write('spec/manifest.schema.json', manifest)
# Manifest CDDL is an imported ABI fixture rule, sharing common definitions.
rule('manifest', obj({'version': {'const': '0.1'}, 'id': ref('coglet-id'), **{k: arr(ref('capability-name')) for k in ['subscriptions', 'required_capabilities', 'exclusive_outputs']}, 'budgets': local('budgets')}), '{version: "0.1", id: coglet-id, subscriptions: [* capability-name], required_capabilities: [* capability-name], exclusive_outputs: [* capability-name], budgets: budgets}')
ROOT_RULES = ['trace', 'step-record', 'boundary-record', 'inbox-message', 'timer-request', 'llm-request', 'llm-reply', 'log-entry', 'manifest']
write('spec/messages/abi.schema.json', {'$schema': 'https://json-schema.org/draft/2020-12/schema', '$id': ABI, 'anyOf': [local(k) for k in ROOT_RULES], '$defs': DEFS})
(ROOT / 'spec/messages/abi.cddl').write_text('; Import common.cddl after this file. Shapes only; semantic invariants are in abi.md.\nabi = ' + ' / '.join(ROOT_RULES) + '\n' + '\n'.join(CDDL) + '\n')

COG = 'coglet:' + '1'*64
SURFACE = 'surface:' + '2'*64
HUB = 'hub:' + '3'*64
ID = '4'*32
budget = dict(compute=10000, compute_mode='fixture.interpreter', memory_pages=2, wall_time_us=100000, max_events=16, host_bytes=65536)
man = dict(version='0.1', id=COG, subscriptions=['sensor.tick'], required_capabilities=['motor.move', 'llm.complete'], exclusive_outputs=['motor.move'], budgets=budget)
event = dict(source=SURFACE, kind='sensor.tick', seq=0, logical_time=10, payload={}, correlation_id=ID)
files = {'/code/bin/step.wasm': '0061736d01000000', '/data/count': '30'}
code = bytes.fromhex(files['/code/bin/step.wasm'])
manifest_bytes = encode(man)
codehash = hashlib.sha256(code).hexdigest()
manhash = hashlib.sha256(manifest_bytes).hexdigest()
# Test-only identity transform provenance, not a qualified production build.
provenance = dict(version='0.1', source_commit='a'*40,
                  recipe_sha256=hashlib.sha256(b'fixture recipe').hexdigest(),
                  policy_sha256=hashlib.sha256(b'test-only policy').hexdigest(),
                  transform_sha256=hashlib.sha256(b'test-only identity transform').hexdigest(),
                  original_sha256=codehash, rewritten_sha256=codehash)
files['/code/build/provenance.cbor'] = encode(provenance).hex()
base = dict(version='0.1', trace_id='5'*32, coglet=COG, step=0, logical_time=10, seed=7, previous=None,
            code_hash=codehash, manifest_hash=manhash, before_hash=digest(files), input_hash=digest(files), output_hash=digest(files),
            engine=dict(build='fixture-v1', policy='TBD-by-ADR-0003'), budgets=budget, grants=man['required_capabilities'], events=[event],
            boundary=[], calls=[], actions=[], timers=[], llm_requests=[], logs=[], input_files=files, output_files=files,
            usage=dict(compute=10, memory_pages=1, wall_time_us=20, host_bytes=0), status='committed')

def seal(rec):
    rec = copy.deepcopy(rec)
    rec.pop('record_hash', None)
    rec['record_hash'] = digest(rec)
    return rec

base = seal(base)
timer = dict(id=ID, at=500010)
llm = dict(id=ID, correlation_id=ID, capability='llm.complete', model='small', input=dict(prompt='hello'))
log = dict(level='info', message='wake', fields={})
action = dict(id=ID, coglet=COG, capability='motor.move', args=dict(x=1), deadline=100, correlation_id=ID)
llm_reply = dict(request_id=ID, status='succeeded', output=dict(text='hello'))
inbox = dict(index=0, id=ID, sender=HUB, event=dict(event, source=HUB, kind='hub.steer', payload=dict(text='hello')))
empty_ledger = dict(actions=[], timers=[], llm_requests=[])
trace = dict(version='0.1', trace_id=base['trace_id'], coglet=COG, initial_effect_ids=empty_ledger, initial_actions=[], initial_inbox_wakes=[], initial_hash=base['before_hash'], initial_files=files, code_hash=codehash, manifest_hash=manhash, final_hash=base['output_hash'], trailing_boundary=None, initial_timers=[], initial_llm_requests=[], records=[base], blobs={codehash: code.hex(), manhash: manifest_bytes.hex()})
valids = {'timer-request': timer, 'llm-request': llm, 'llm-reply': llm_reply, 'log-entry': log, 'inbox-message': inbox, 'budgets': budget, 'snapshot': files, 'call': dict(op='timer', argument=timer, result=0), 'engine': base['engine'], 'usage': base['usage'], 'step-record': base, 'trace': trace, 'manifest': man, 'boundary-change': dict(actor=HUB, audit_id=ID, ticket=None, method='debug.write', target=COG, path='/data/count', before='30', after='31'), 'failure': dict(kind='wall', error=dict(code='deadline_exceeded', message='step expired', retryable=False))}
valids['action-rejection'] = dict(id=ID, correlation_id=ID, accepted=False,
    error=dict(code='permission_denied', message='grant revoked', retryable=False))
valids['boundary-record'] = seal(dict(version='0.1', trace_id=base['trace_id'], coglet=COG,
    logical_time=20, previous=None, before_hash=digest(files),
    output_hash=digest(dict(files, **{'/data/count': '31'})),
    boundary=[valids['boundary-change']], output_files=dict(files, **{'/data/count': '31'})))
cases = []
for typ, value in valids.items():
    cases.append(dict(name=typ+'-valid', type=typ, valid=True, value=value, cbor_hex=encode(value).hex()))
    bad = copy.deepcopy(value)
    if typ == 'snapshot': bad['/data/invalid'] = '0'
    else: bad['unknown'] = True
    cases.append(dict(name=typ+'-unknown-or-path-invalid', type=typ, valid=False, value=bad))
for name, typ, value in [('zero-compute', 'budgets', dict(budget, compute=0)), ('float-time', 'timer-request', dict(timer, at=0.5)), ('odd-file-hex', 'snapshot', {'/data/count': '0'}), ('failed-reply-with-output', 'llm-reply', dict(inbox['event']['payload'], status='failed'))]:
    cases.append(dict(name=name, type=typ, valid=False, value=value))
for label, path in [('ascii', '/data/count'), ('utf8', '/data/café')]:
    cases.append(dict(name='path-' + label, type='path', valid=True, value=path, cbor_hex=encode(path).hex()))
for codepoint in list(range(32)) + [127, 92]:
    path = '/data/a' + chr(codepoint) + 'b'
    cases.append(dict(name=f'path-forbidden-{codepoint}', type='path', valid=False, value=path))
    change = dict(valids['boundary-change'], path=path)
    cases.append(dict(name=f'boundary-path-forbidden-{codepoint}', type='boundary-change', valid=False, value=change))
write('spec/fixtures/abi/cases.json', cases)

# Each timeline is an isolated adapter reset/step invocation. The scripted calls
# are the language-neutral mock-engine input; adapters supply equivalent guests.
timelines = []
records = []
for name in ['debug-step', 'no-effects', 'action', 'timer', 'llm', 'log', 'abort-wall', 'abort-compute', 'abort-memory', 'abort-host_bytes', 'inbox-boundary', 'invalid-call', 'duplicate-timer', 'denied-action']:
    rec = copy.deepcopy(base)
    if name == 'debug-step':
        rec['events'] = [dict(event, source=COG, kind='runtime.debug_step', payload={})]
    elif name == 'invalid-call':
        rec['calls'] = [dict(op='timer', argument_hex='ff', result=1)]
    elif name == 'duplicate-timer':
        rec['calls'] = [dict(op='timer', argument=timer, result=0), dict(op='timer', argument=timer, result=4)]
        rec['timers'] = [timer]
    elif name == 'denied-action':
        rec['calls'] = [dict(op='emit', argument=dict(action, capability='other.write'), result=2)]
    elif name in ['action', 'timer', 'llm', 'log']:
        op, arg, field = {'action': ('emit', action, 'actions'), 'timer': ('timer', timer, 'timers'), 'llm': ('llm', llm, 'llm_requests'), 'log': ('log', log, 'logs')}[name]
        rec['calls'] = [dict(op=op, argument=arg, result=0)]
        rec[field] = [arg]
        if name == 'log':
            out = dict(files)
            out['/logs/events.jsonl'] = (json.dumps(log, sort_keys=True, separators=(',', ':'))+'\n').encode().hex()
            rec['output_files'] = out
            rec['output_hash'] = digest(out)
    elif name.startswith('abort'):
        kind = name.split('-')[1]
        rec.update(status='aborted', failure=dict(kind=kind, error=dict(code='deadline_exceeded' if kind == 'wall' else 'resource_exhausted', message='step budget exceeded', retryable=False)))
        rec['calls'] = [dict(op='emit', argument=action, result=0)]
    elif name == 'inbox-boundary':
        out = dict(files)
        out['/data/inbox.jsonl'] = (json.dumps(inbox, sort_keys=True, separators=(',', ':'), ensure_ascii=False)+'\n').encode().hex()
        rec['boundary'] = [dict(actor=HUB, audit_id=ID, ticket=None, method='hub.inbox', target=COG, path='/data/inbox.jsonl', before=None, after=out['/data/inbox.jsonl'])]
        rec.update(input_files=out, output_files=out, input_hash=digest(out), output_hash=digest(out))
        rec['events'] = [dict(event, source=COG, kind='runtime.inbox', payload=dict(index=0, id=ID))]
    rec = seal(rec)
    records.append((name, rec))
    send = dict(operation='abi.step', initial_files=files, manifest=man,
                inputs={k: rec[k] for k in ['trace_id', 'coglet', 'step', 'logical_time', 'seed', 'previous', 'engine', 'budgets', 'grants', 'events', 'boundary']},
                guest_script=dict(calls=rec['calls'], writes={}, termination=rec['status'], usage=rec['usage']))
    if rec['status'] == 'aborted':
        send['guest_script']['failure'] = rec['failure']
    timelines.append(dict(name=name, steps=[dict(send=send), dict(expect=dict(journal=rec, state_hash=rec['output_hash'], record_hash=rec['record_hash']), within_ms=200), dict(journal_hash=rec['record_hash'], path='record_hash')]))
write('conformance/abi/cases.json', timelines)
semantics = []
for name, rec in records:
    semantics.append(dict(name=name+'-valid', valid=True, value=rec))
    bad = copy.deepcopy(rec)
    bad['output_hash'] = '0'*64
    bad = seal(bad)
    semantics.append(dict(name=name+'-bad-tree-hash', valid=False, value=bad))
for name, mutate in [('effect-on-abort', lambda r: r.update(status='aborted', failure=valids['failure'], actions=[action])), ('wrong-effect-list', lambda r: r.update(actions=[action])), ('unsafe-path', lambda r: r['output_files'].update({'/data/../secret': ''})), ('boundary-before-mismatch', lambda r: r.update(boundary=[valids['boundary-change']]))]:
    bad = copy.deepcopy(base)
    mutate(bad)
    semantics.append(dict(name=name, valid=False, value=seal(bad)))
for name, source, mutate in [
    ('code-write', base, lambda r: r['output_files'].update({'/code/bin/step.wasm': '00'})),
    ('log-effect-missing-bytes', dict(records)['log'], lambda r: r['output_files'].pop('/logs/events.jsonl')),
    ('wall-error-code', dict(records)['abort-wall'], lambda r: r['failure']['error'].update(code='internal')),
    ('oversized-batch', base, lambda r: r['budgets'].update(max_events=0)),
    ('unsafe-path-rehashed', base, lambda r: r['output_files'].update({'/data/../secret': ''})),
]:
    bad = copy.deepcopy(source)
    # JSON copy breaks aliases, as real archived JSON does.
    bad = json.loads(json.dumps(bad))
    mutate(bad)
    bad['output_hash'] = digest(bad['output_files'])
    semantics.append(dict(name=name, valid=False, value=seal(bad)))
write('spec/fixtures/abi/semantic-cases.fixture', semantics)

# Full two-wake recording: async request, then inbox reply. No live LLM needed.
request_record = dict(records)['llm']
reply_record = copy.deepcopy(base)
reply_inbox = copy.deepcopy(inbox)
reply_inbox['event'].update(kind='llm.reply', payload=llm_reply, seq=1, logical_time=20)
reply_files = dict(files)
reply_files['/data/inbox.jsonl'] = (json.dumps(reply_inbox, sort_keys=True, separators=(',', ':'), ensure_ascii=False)+'\n').encode().hex()
reply_record.update(step=1, logical_time=20, previous=request_record['record_hash'], input_files=reply_files,
                    output_files=reply_files, input_hash=digest(reply_files), output_hash=digest(reply_files),
                    events=[dict(event, source=COG, seq=1, kind='runtime.inbox', payload=dict(index=0, id=ID), logical_time=20)],
                    boundary=[dict(actor=HUB, audit_id='6'*32, ticket=None, method='hub.inbox', target=COG, path='/data/inbox.jsonl', before=None, after=reply_files['/data/inbox.jsonl'])])
reply_record = seal(reply_record)
reply_trace = dict(trace, records=[request_record, reply_record])
write('spec/fixtures/abi/llm-reply-trace.fixture', reply_trace)
reply_case = dict(name='async-llm-two-wakes', steps=[])
for index, rec in enumerate(reply_trace['records']):
    send = dict(operation='abi.step', initial_files=files if index == 0 else reply_trace['records'][index-1]['output_files'], manifest=man,
                inputs={k: rec[k] for k in ['trace_id', 'coglet', 'step', 'logical_time', 'seed', 'previous', 'engine', 'budgets', 'grants', 'events', 'boundary']},
                guest_script=dict(calls=rec['calls'], writes={}, termination=rec['status'], usage=rec['usage']))
    reply_case['steps'].extend([dict(send=send), dict(expect=dict(journal=rec, state_hash=rec['output_hash'], record_hash=rec['record_hash']), within_ms=200), dict(journal_hash=rec['record_hash'], path='record_hash')])
timelines.append(reply_case)
write('conformance/abi/cases.json', timelines)
# Rehashed semantic regressions: container shapes/hashes stay valid.
trace_cases = []
def trace_case(name, value, valid=True, error=None):
    item = dict(name=name, value=copy.deepcopy(value), valid=valid)
    if error:
        item['error'] = error
    trace_cases.append(item)

# Provenance negatives rehash every enclosing tree/record: digest failures must
# not conceal absent, malformed or wrong-output provenance.
provenance_cases = [dict(name='provenance-control', value=copy.deepcopy(trace), valid=True)]
for name, replacement in [
    ('missing', None),
    ('wrong-output', encode(dict(provenance, rewritten_sha256='0'*64)).hex()),
    ('missing-policy', encode({k: v for k, v in provenance.items() if k != 'policy_sha256'}).hex()),
    ('unknown-field', encode(dict(provenance, unknown=True)).hex()),
    ('malformed-cbor', 'a1'),
    ('trailing-cbor', encode(provenance).hex() + '00'),
    ('noncanonical-cbor', 'b807' + encode(provenance).hex()[2:]),
]:
    sample = json.loads(json.dumps(trace))
    rec = sample['records'][0]
    for side in ('input', 'output'):
        snapshot = rec[side + '_files']
        if replacement is None:
            snapshot.pop('/code/build/provenance.cbor')
        else:
            snapshot['/code/build/provenance.cbor'] = replacement
        rec[side + '_hash'] = digest(snapshot)
    rec['before_hash'] = rec['input_hash']
    sample['initial_hash'] = rec['before_hash']
    sample['records'][0] = seal(rec)
    provenance_cases.append(dict(name='provenance-' + name, value=sample, valid=False))
write('spec/fixtures/abi/provenance-cases.fixture', provenance_cases)
write('conformance/abi/provenance.json', [dict(name=c['name'], steps=[
    dict(send=dict(operation='abi.validate_trace', trace=c['value'])),
    dict(expect=dict(valid=c['valid'], dispatched=False), within_ms=200),
]) for c in provenance_cases])

trace_case('async-reply-control', reply_trace)
debug_trace = dict(trace, records=[dict(records)['debug-step']])
trace_case('debug-step-control', debug_trace)
for field, value in [('source', SURFACE), ('logical_time', 9), ('payload', {'untrusted': True})]:
    bad = copy.deepcopy(debug_trace)
    bad['records'][0]['events'][0][field] = value
    bad['records'][0] = seal(bad['records'][0])
    trace_case('debug-step-wrong-'+field, bad, False, 'debug step wake binding')
for name, error in [('reply-missing-output', 'embedded llm-reply'),
                    ('inbox-missing-sender', 'embedded inbox-message'),
                    ('inbox-spaces', 'noncanonical inbox'),
                    ('inbox-crlf', 'noncanonical inbox'),
                    ('wake-wrong-id', 'inbox wake binding'),
                    ('wake-wrong-correlation', 'inbox wake binding'),
                    ('wake-duplicate', 'unscheduled inbox wake')]:
    bad = json.loads(json.dumps(reply_trace))
    rec = bad['records'][1]
    message = json.loads(bytes.fromhex(rec['input_files']['/data/inbox.jsonl']))
    if name == 'reply-missing-output':
        del message['event']['payload']['output']
    if name == 'inbox-missing-sender':
        del message['sender']
    text = json.dumps(message, sort_keys=True, separators=(',', ':') if name != 'inbox-spaces' else (', ', ': '), ensure_ascii=False)
    raw = (text + ('\r\n' if name == 'inbox-crlf' else '\n')).encode().hex()
    rec['input_files']['/data/inbox.jsonl'] = raw
    rec['output_files']['/data/inbox.jsonl'] = raw
    rec['boundary'][0]['after'] = raw
    if name == 'wake-wrong-id':
        rec['events'][0]['payload']['id'] = '0'*32
    if name == 'wake-wrong-correlation':
        rec['events'][0]['correlation_id'] = '0'*32
    if name == 'wake-duplicate':
        rec['events'].append(copy.deepcopy(rec['events'][0]))
    rec['input_hash'] = digest(rec['input_files'])
    rec['output_hash'] = digest(rec['output_files'])
    bad['records'][1] = seal(rec)
    trace_case(name, bad, False, error)

# A full batch may defer an appended line's wake to a subsequent step.
deferred = copy.deepcopy(reply_trace)
deferred['records'][1]['events'] = [dict(event, seq=1, logical_time=20)]
deferred['records'][1] = seal(deferred['records'][1])
later = copy.deepcopy(reply_record)
later.update(step=2, previous=deferred['records'][1]['record_hash'], boundary=[], before_hash=later['input_hash'])
later['events'][0]['seq'] = 2
deferred['records'].append(seal(later))
trace_case('deferred-inbox-wake', deferred)
# Same deferred wake when a trace starts after the append (e.g. promotion).
initial_wake = copy.deepcopy(trace)
rec = copy.deepcopy(later)
rec.update(step=0, previous=None)
initial_wake.update(initial_hash=rec['before_hash'], initial_inbox_wakes=[0], records=[seal(rec)])
trace_case('initial-pending-inbox-wake', initial_wake)
missing = copy.deepcopy(initial_wake)
missing['initial_inbox_wakes'] = []
trace_case('initial-wake-not-pending', missing, False, 'unscheduled inbox wake')

# Completed effects survive promotion even though pending timer/LLM arrays are empty.
for op, kind, argument in [('emit', 'actions', action), ('timer', 'timers', timer), ('llm', 'llm_requests', llm)]:
    for historical in (False, True):
        name = ('historical-' if historical else 'fresh-') + op + '-id'
        ledger = copy.deepcopy(empty_ledger)
        if historical:
            ledger[kind] = [ID]
        rec = copy.deepcopy(base)
        rec['calls'] = [dict(op=op, argument=argument, result=4 if historical else 0)]
        rec[kind] = [] if historical else [argument]
        rec = seal(rec)
        sample = dict(trace, initial_effect_ids=ledger, records=[rec])
        trace_case(name, sample)
        bad = copy.deepcopy(sample)
        bad['records'][0]['calls'][0]['result'] = 0 if historical else 4
        bad['records'][0][kind] = [argument] if historical else []
        bad['records'][0] = seal(bad['records'][0])
        trace_case(name+'-wrong-return', bad, False, 'duplicate effect id accepted' if historical else 'conflict without prior effect id')
        send = dict(operation='abi.step', initial_files=files, manifest=man,
                    initial_effect_ids=ledger, initial_inbox_wakes=[], initial_timers=[], initial_llm_requests=[],
                    inputs={k: rec[k] for k in ['trace_id', 'coglet', 'step', 'logical_time', 'seed', 'previous', 'engine', 'budgets', 'grants', 'events', 'boundary']},
                    guest_script=dict(calls=rec['calls'], writes={}, termination=rec['status'], usage=rec['usage']))
        timelines.append(dict(name=name, steps=[dict(send=send), dict(expect=dict(journal=rec, state_hash=rec['output_hash'], record_hash=rec['record_hash']), within_ms=200), dict(journal_hash=rec['record_hash'], path='record_hash')]))
# Result precedence is independently checkable even without an engine's resource meter.
for op, kind, argument in [('emit', 'actions', action), ('timer', 'timers', timer), ('llm', 'llm_requests', llm)]:
    for scope in ('historical', 'same-step'):
        ledger = copy.deepcopy(empty_ledger)
        rec = copy.deepcopy(base)
        if scope == 'historical':
            ledger[kind] = [ID]
        else:
            rec['calls'].append(dict(op=op, argument=argument, result=0))
            rec[kind] = [argument]
        rec['calls'].append(dict(op=op, argument=argument, result=4))
        sample = dict(trace, initial_effect_ids=ledger, records=[seal(rec)])
        trace_case(scope+'-'+op+'-conflict-control', sample)
        for result in (1, 2, 3):
            bad = copy.deepcopy(sample)
            bad['records'][0]['calls'][-1]['result'] = result
            bad['records'][0] = seal(bad['records'][0])
            trace_case(scope+'-'+op+'-wrong-result-'+str(result), bad, False, 'call result precedence')
    for result in (0, 3):
        rec = copy.deepcopy(base)
        rec['calls'] = [dict(op=op, argument=argument, result=result)]
        rec[kind] = [argument] if result == 0 else []
        trace_case('fresh-'+op+'-result-'+str(result), dict(trace, records=[seal(rec)]))
    if op in ('emit', 'llm'):
        for reason in (('capability', 'identity') if op == 'emit' else ('capability',)):
            denied = dict(argument)
            denied['capability' if reason == 'capability' else 'coglet'] = 'other.write' if reason == 'capability' else 'coglet:'+'9'*64
            for historical in (False, True):
                ledger = copy.deepcopy(empty_ledger)
                if historical:
                    ledger[kind] = [ID]
                for result in (1, 2, 3, 4):
                    rec = copy.deepcopy(base)
                    rec['calls'] = [dict(op=op, argument=denied, result=result)]
                    trace_case(f'{op}-{reason}-denied-{historical}-result-{result}',
                               dict(trace, initial_effect_ids=ledger, records=[seal(rec)]),
                               result == 2, None if result == 2 else 'call result precedence')
# Initial pending work must be present in the complete historical ledger.
invalid = dict(trace, initial_timers=[timer])
trace_case('pending-timer-missing-ledger', invalid, False, 'initial pending work')
valid_pending = dict(invalid, initial_effect_ids=dict(empty_ledger, timers=[ID]))
trace_case('pending-timer-ledger-control', valid_pending)
for timeline in timelines:
    first = timeline['steps'][0]['send']
    for key, value in [('initial_effect_ids', empty_ledger), ('initial_inbox_wakes', []), ('initial_timers', []), ('initial_llm_requests', [])]:
        first.setdefault(key, copy.deepcopy(value))
write('conformance/abi/cases.json', timelines)
write('spec/fixtures/abi/trace-cases.fixture', trace_cases)
# Real core-Wasm bytes for pre-instantiation admission (not scripted outcomes).
# Two functions: abi_alloc(i32)->i32 and handle(i32,i32,i64,i64,i64)->i32.
# Both return zero; these cases never call them. Each section fits one-byte LEB.
def admission_module(initial, maximum):
    def section(number, payload):
        return bytes([number, len(payload)]) + payload
    types = bytes.fromhex('0260017f017f60057f7f7e7e7e017f')
    exports = bytes.fromhex('03066d656d6f72790200096162695f616c6c6f6300000668616e646c650001')
    return (bytes.fromhex('0061736d01000000') + section(1, types)
            + section(3, bytes([2, 0, 1]))
            + section(5, bytes([1, 1, initial, maximum]))
            + section(7, exports) + section(10, bytes.fromhex('02040041000b040041000b')))

admission_cases = []
for name, initial, maximum, admitted in [('initial-memory-at-cap', 2, 2, True),
                                        ('initial-memory-over-cap', 3, 3, False),
                                        ('maximum-memory-over-cap', 2, 3, False)]:
    module = admission_module(initial, maximum)
    initial_files = dict(files, **{'/code/bin/step.wasm': module.hex(),
        '/code/build/provenance.cbor': encode(dict(provenance,
            original_sha256=hashlib.sha256(module).hexdigest(),
            rewritten_sha256=hashlib.sha256(module).hexdigest())).hex()})
    response = dict(admitted=admitted, instantiated=admitted, handle_calls=0,
                    effective_memory_pages=budget['memory_pages'],
                    pending_events=[event], journal=None, record_hash=None,
                    state_hash=digest(initial_files))
    if not admitted:
        response['error'] = dict(code='resource_exhausted', message='module memory exceeds granted cap', retryable=False)
    admission_cases.append(dict(name=name, steps=[
        dict(send=dict(operation='abi.admit', initial_files=initial_files, manifest=man,
                       budgets=budget, pending_events=[event])),
        dict(expect=response, within_ms=200),
        dict(journal_hash=response['state_hash'], path='state_hash')]))
write('conformance/abi/admission.json', admission_cases)
print(f'generated {len(cases)} shape cases, {len(semantics)} semantic cases, {len(trace_cases)} trace cases, {len(timelines)} step timelines, {len(admission_cases)} admission timelines')
