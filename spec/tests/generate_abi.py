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
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + '\n')


def digest(value):
    return hashlib.sha256(encode(value)).hexdigest()


U = ref('uint64')
S = {'type': 'string', 'minLength': 1}
HEX = {'type': 'string', 'pattern': '^([0-9a-f]{2})*(?![\\s\\S])'}
H = {'type': 'string', 'pattern': '^[0-9a-f]{64}(?![\\s\\S])'}
PATH = {'type': 'string', 'pattern': '^/(code|data|logs)(/[^/\u0000]+)+(?![\\s\\S])'}
rule('hash', H, 'tstr .regexp "^[0-9a-f]{64}$"')
rule('hex', HEX, 'tstr .regexp "^([0-9a-f]{2})*$"')
rule('path', PATH, 'tstr .regexp "^/(code|data|logs)(/[^/]+)+$"')
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
rule('call', {'oneOf': variants}, ' / '.join(texts))
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
record('trace', fields + [('initial_hash', local('hash'), 'hash'), ('initial_timers', arr(local('timer-request')), '[* timer-request]'), ('initial_llm_requests', arr(local('llm-request')), '[* llm-request]'), ('records', arr(local('step-record'), 1), '[+ step-record]'), ('blobs', {'type': 'object', 'additionalProperties': local('hex')}, '{* tstr => hex}')])
manifest = obj({'version': {'const': '0.1'}, 'id': ref('coglet-id'),
                **{k: dict(arr(ref('capability-name')), uniqueItems=True) for k in ['subscriptions', 'required_capabilities', 'exclusive_outputs']},
                'budgets': {'$ref': ABI + '#/$defs/budgets'}})
manifest.update({'$schema': 'https://json-schema.org/draft/2020-12/schema', '$id': BASE + 'manifest.schema.json'})
write('spec/manifest.schema.json', manifest)
# Manifest CDDL is an imported ABI fixture rule, sharing common definitions.
rule('manifest', obj({'version': {'const': '0.1'}, 'id': ref('coglet-id'), **{k: arr(ref('capability-name')) for k in ['subscriptions', 'required_capabilities', 'exclusive_outputs']}, 'budgets': local('budgets')}), '{version: "0.1", id: coglet-id, subscriptions: [* capability-name], required_capabilities: [* capability-name], exclusive_outputs: [* capability-name], budgets: budgets}')
write('spec/messages/abi.schema.json', {'$schema': 'https://json-schema.org/draft/2020-12/schema', '$id': ABI, 'anyOf': [local(k) for k in DEFS], '$defs': DEFS})
(ROOT / 'spec/messages/abi.cddl').write_text('; Import common.cddl after this file. Shapes only; semantic invariants are in abi.md.\nabi = trace / step-record / inbox-message / timer-request / llm-request / llm-reply / log-entry / manifest\n' + '\n'.join(CDDL) + '\n')

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
trace = dict(version='0.1', trace_id=base['trace_id'], coglet=COG, initial_hash=base['before_hash'], initial_timers=[], initial_llm_requests=[], records=[base], blobs={codehash: code.hex(), manhash: manifest_bytes.hex()})
valids = {'timer-request': timer, 'llm-request': llm, 'llm-reply': llm_reply, 'log-entry': log, 'inbox-message': inbox, 'budgets': budget, 'snapshot': files, 'call': dict(op='timer', argument=timer, result=0), 'engine': base['engine'], 'usage': base['usage'], 'step-record': base, 'trace': trace, 'manifest': man, 'boundary-change': dict(actor=HUB, audit_id=ID, ticket=None, method='debug.data.write', target=COG, path='/data/count', before='30', after='31'), 'failure': dict(kind='wall', error=dict(code='deadline_exceeded', message='step expired', retryable=False))}
cases = []
for typ, value in valids.items():
    cases.append(dict(name=typ+'-valid', type=typ, valid=True, value=value, cbor_hex=encode(value).hex()))
    bad = copy.deepcopy(value)
    if typ == 'snapshot': bad['/data/invalid'] = '0'
    else: bad['unknown'] = True
    cases.append(dict(name=typ+'-unknown-or-path-invalid', type=typ, valid=False, value=bad))
for name, typ, value in [('zero-compute', 'budgets', dict(budget, compute=0)), ('float-time', 'timer-request', dict(timer, at=0.5)), ('odd-file-hex', 'snapshot', {'/data/count': '0'}), ('failed-reply-with-output', 'llm-reply', dict(inbox['event']['payload'], status='failed'))]:
    cases.append(dict(name=name, type=typ, valid=False, value=value))
write('spec/fixtures/abi/cases.json', cases)

# Each timeline is an isolated adapter reset/step invocation. The scripted calls
# are the language-neutral mock-engine input; adapters supply equivalent guests.
timelines = []
records = []
for name in ['no-effects', 'action', 'timer', 'llm', 'log', 'abort-wall', 'abort-compute', 'abort-memory', 'abort-host_bytes', 'inbox-boundary', 'invalid-call', 'duplicate-timer', 'denied-action']:
    rec = copy.deepcopy(base)
    if name == 'invalid-call':
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
        rec['boundary'] = [dict(actor=HUB, audit_id=ID, ticket=None, method='inbox.append', target=COG, path='/data/inbox.jsonl', before=None, after=out['/data/inbox.jsonl'])]
        rec.update(input_files=out, output_files=out, input_hash=digest(out), output_hash=digest(out))
        rec['events'] = [dict(event, kind='runtime.inbox', payload=dict(index=0, id=ID))]
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
write('spec/fixtures/abi/semantic-cases.json', semantics)

# Full two-wake recording: async request, then inbox reply. No live LLM needed.
request_record = dict(records)['llm']
reply_record = copy.deepcopy(base)
reply_inbox = copy.deepcopy(inbox)
reply_inbox['event'].update(kind='llm.reply', payload=llm_reply, seq=1, logical_time=20)
reply_files = dict(files)
reply_files['/data/inbox.jsonl'] = (json.dumps(reply_inbox, sort_keys=True, separators=(',', ':'), ensure_ascii=False)+'\n').encode().hex()
reply_record.update(step=1, logical_time=20, previous=request_record['record_hash'], input_files=reply_files,
                    output_files=reply_files, input_hash=digest(reply_files), output_hash=digest(reply_files),
                    events=[dict(event, seq=1, kind='runtime.inbox', payload=dict(index=0, id=ID), logical_time=20)],
                    boundary=[dict(actor=HUB, audit_id='6'*32, ticket=None, method='inbox.append', target=COG, path='/data/inbox.jsonl', before=None, after=reply_files['/data/inbox.jsonl'])])
reply_record = seal(reply_record)
reply_trace = dict(trace, records=[request_record, reply_record])
write('spec/fixtures/abi/llm-reply-trace.json', reply_trace)
reply_case = dict(name='async-llm-two-wakes', steps=[])
for index, rec in enumerate(reply_trace['records']):
    send = dict(operation='abi.step', initial_files=files if index == 0 else reply_trace['records'][index-1]['output_files'], manifest=man,
                inputs={k: rec[k] for k in ['trace_id', 'coglet', 'step', 'logical_time', 'seed', 'previous', 'engine', 'budgets', 'grants', 'events', 'boundary']},
                guest_script=dict(calls=rec['calls'], writes={}, termination=rec['status'], usage=rec['usage']))
    reply_case['steps'].extend([dict(send=send), dict(expect=dict(journal=rec, state_hash=rec['output_hash'], record_hash=rec['record_hash']), within_ms=200), dict(journal_hash=rec['record_hash'], path='record_hash')])
timelines.append(reply_case)
write('conformance/abi/cases.json', timelines)
print(f'generated {len(cases)} shape cases, {len(semantics)} semantic cases, {len(timelines)} conformance timelines')
