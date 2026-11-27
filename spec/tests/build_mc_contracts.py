"""Generate M-C executable case data, not implementation qualification."""
import copy
import json
from test_common import SPEC, encode
from test_abi import digest, record_hash
from heard_archive import HEARD_PATH


def dump(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + '\n')


def relay_case():
    case = json.loads((SPEC.parent / 'conformance/hub/event-batch-dedup.json').read_text())
    case['name'] = 'event-retry-a-after-b'
    case['fixtures'] = ['hub.event-retry-a-after-b']
    a = copy.deepcopy(case['steps'][0]['send'])
    b = copy.deepcopy(a)
    b['correlation_id'] = '2'*32
    b['params']['offset'] = 1
    b['params']['events'][0]['seq'] = 1
    b['params']['events'][0]['payload'] = {'batch': 'B'}
    retry = copy.deepcopy(a)
    retry['correlation_id'] = '3'*32
    case['steps'] = []
    for request, position in [(a, 1), (b, 2), (retry, 1)]:
        case['steps'] += [dict(send=request), dict(match={'correlation_id':request['correlation_id'],
                                                        'result.next_offset':position}, within_ms=2000)]
    return case


def clock_vectors():
    return [dict(name='abi-to-surface-different-epochs', origin='abi', deadline=1500,
                 source_now=1000, runtime_now=1000, surface_now=9000000,
                 runtime_deadline=1500, surface_deadline=9000500),
            dict(name='hub-actions-receiver-runtime-epoch', origin='hub.actions', deadline=9000500,
                 source_now=9000000, runtime_now=1000, surface_now=42000,
                 runtime_deadline=1500, surface_deadline=42500)]


def heard_vectors():
    shapes = json.loads((SPEC / 'fixtures/abi/cases.json').read_text())
    base = copy.deepcopy(next(c['value'] for c in shapes if c['type']=='trace' and c['valid']))
    surface = 'surface:'+'2'*64
    def item(number, played, stopped=False, spans=None):
        spans = spans if spans is not None else [dict(end_ms=100, text='Hello '), dict(end_ms=300, text='world')]
        return dict(surface=surface, item=f'{number:032x}', duration_ms=400,
                    played_ms=played, stopped=stopped, spans=spans,
                    heard=''.join(s['text'] for s in spans if s['end_ms'] <= played).encode()[:16384].decode(errors='ignore'))
    state = dict(items=[item(1,400,True), item(2,150), item(3,0), item(4,300,spans=[])])
    advanced = copy.deepcopy(state)
    advanced['items'][1].update(played_ms=300, heard='Hello world', stopped=True)

    def build(states, prestep=False, initial=None):
        trace = copy.deepcopy(base)
        trace['records'] = []
        if initial is not None:
            trace['initial_files'][HEARD_PATH] = encode(initial).hex()
            trace['initial_hash'] = digest(trace['initial_files'])
        files = copy.deepcopy(trace['initial_files'])
        changes = []
        for n, value in enumerate(states):
            after = encode(value).hex()
            changes.append(dict(actor=trace['coglet'], target=trace['coglet'], ticket=None,
                                method='runtime.heard', audit_id=f'{n+1:032x}', path=HEARD_PATH,
                                before=files.get(HEARD_PATH), after=after))
            files[HEARD_PATH] = after
        if prestep:
            rec = copy.deepcopy(base['records'][0])
            rec.update(boundary=changes, before_hash=trace['initial_hash'], input_files=copy.deepcopy(files),
                       output_files=copy.deepcopy(files), input_hash=digest(files), output_hash=digest(files))
            rec['record_hash'] = record_hash(rec)
            trace['records'] = [rec]
        elif changes:
            rec = dict(version='1.0',trace_id=trace['trace_id'],coglet=trace['coglet'],logical_time=20,
                       previous=None,before_hash=trace['initial_hash'],output_hash=digest(files),
                       boundary=changes,output_files=files)
            rec['record_hash'] = record_hash(rec)
            trace['trailing_boundary'] = rec
        trace['final_hash'] = digest(files)
        return trace

    vectors = []
    def add(name, trace, error=None):
        vectors.append(dict(name=name, value=trace, valid=error is None, **({'error':error} if error else {})))
    add('heard-export-replay-journal-alone', build([state,advanced]))
    add('heard-before-reasoning-step', build([state],prestep=True))
    add('heard-continuation-initial-state', build([],initial=advanced))
    aligned_later = copy.deepcopy(state)
    aligned_later['items'][3].update(spans=copy.deepcopy(state['items'][0]['spans']), heard='Hello world')
    add('heard-late-measured-alignment', build([state,aligned_later]))
    add('heard-utf8-prefix-cap', build([dict(items=[item(1,400,True,[dict(end_ms=100,text='é'*8191+'🙂tail')])])]))
    for name, mutation, error in [
        ('unheard-prefix', lambda s: s['items'][1].update(heard='Hello world'), 'heard state: prefix'),
        ('queued-text', lambda s: s['items'][2].update(heard='Hello '), 'heard state: prefix'),
        ('no-alignment-text', lambda s: s['items'][3].update(heard='guessed'), 'heard state: prefix'),
        ('played-past-duration', lambda s: s['items'][1].update(played_ms=401), 'heard state: played'),
        ('duplicate-item', lambda s: s['items'].append(copy.deepcopy(s['items'][0])), 'heard state: item'),
        ('alignment-order', lambda s: s['items'][1]['spans'].reverse(), 'heard state: alignment'),
        ('alignment-past-duration', lambda s: s['items'][1]['spans'][1].update(end_ms=500), 'heard state: alignment'),
        ('unknown-state-field', lambda s: s.update(injected=True), 'heard state: shape'),
        ('unknown-item-field', lambda s: s['items'][0].update(injected=True), 'heard state: shape'),
    ]:
        bad = copy.deepcopy(state); mutation(bad)
        add('heard-'+name,build([bad]),error)
    for name, mutation, error in [
        ('position-regression', lambda s: s['items'][1].update(played_ms=100,heard='Hello '), 'heard position regressed'),
        ('stopped-advance', lambda s: s['items'][1].update(played_ms=400), 'heard stopped item advanced'),
        ('stopped-revive', lambda s: s['items'][1].update(stopped=False), 'heard stopped item advanced'),
        ('alignment-rewrite', lambda s: s['items'][1]['spans'][1].update(end_ms=250), 'heard alignment changed'),
        ('item-removal', lambda s: s['items'].pop(), 'heard item removed'),
    ]:
        bad=copy.deepcopy(advanced); mutation(bad)
        add('heard-'+name,build([advanced,bad]),error)
    for name, mutation, error in [
        ('hub-forgery',lambda c:c.update(method='manage.write'), 'heard runtime origin'),
        ('actor-forgery',lambda c:c.update(actor='hub:'+'3'*64), 'heard runtime origin'),
        ('ticket-forgery',lambda c:c.update(ticket='5'*32), 'heard runtime origin'),
        ('wrong-path',lambda c:c.update(path='/data/not-heard'), 'heard runtime origin'),
        ('deletion',lambda c:c.update(after=None), 'heard deletion'),
    ]:
        trace=build([state]); rec=trace['trailing_boundary']; change=rec['boundary'][0]
        rec['output_files'].pop(change['path'])
        mutation(change)
        if change['after'] is not None: rec['output_files'][change['path']]=change['after']
        rec['output_hash']=digest(rec['output_files']); rec['record_hash']=record_hash(rec)
        trace['final_hash']=rec['output_hash']
        add('heard-'+name,trace,error)
    trace=build([state],prestep=True); rec=trace['records'][0]
    rec['output_files'][HEARD_PATH]=encode(advanced).hex()
    rec['output_hash']=digest(rec['output_files']);rec['record_hash']=record_hash(rec);trace['final_hash']=rec['output_hash']
    add('heard-guest-write',trace,'read-only write')
    trace=build([],prestep=True);rec=trace['records'][0]
    rec['output_files'][HEARD_PATH]=encode(state).hex()
    rec['output_hash']=digest(rec['output_files']);rec['record_hash']=record_hash(rec);trace['final_hash']=rec['output_hash']
    add('heard-guest-create',trace,'read-only create')
    return vectors


def outputs():
    vectors=heard_vectors()
    timelines=[]
    for v in vectors:
        timelines.append(dict(name=v['name'],peer_role='runtime',fixtures=['abi.'+v['name']],steps=[
            dict(send=dict(operation='abi.validate_trace',trace=v['value'])),
            dict(match=dict(valid=v['valid'],dispatched=False,engine_invocations=0),within_ms=1000)]))
    return {'conformance/hub/event-retry-a-after-b.json':relay_case(),
            'spec/fixtures/abi/clock-domains.fixture':clock_vectors(),
            'spec/fixtures/abi/heard-archive.fixture':vectors,
            'spec/fixtures/abi/heard-archive-timeline.fixture':timelines}


if __name__=='__main__':
    for path,value in outputs().items(): dump(SPEC.parent/path,value)
