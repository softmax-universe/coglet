"""Generate M-A archive vectors/case contracts, never engine outcomes.

Run after generate_abi.py. Inputs are retained fixture snapshots, not Wasm runs.
"""
import copy
import json
from test_common import SPEC
from test_abi import digest, record_hash


def clone(value):
    return copy.deepcopy(value)


def seal(record):
    record['record_hash'] = record_hash(record)
    return record


def generate(write_files=True):
    shapes = json.loads((SPEC / 'fixtures/abi/cases.json').read_text())
    trace = clone(next(c['value'] for c in shapes if c['type'] == 'trace' and c['valid']))
    base = clone(trace['records'][0])
    coglet = trace['coglet']
    hub = 'hub:' + '3'*64
    surface = 'surface:' + '2'*64
    action = dict(id='4'*32, coglet=coglet, capability='motor.move', args={'x': 1},
                  deadline=100, correlation_id='4'*32)
    vectors = []

    def add(name, sample, error=None, pending=None, wakes=None):
        value = clone(sample)
        item = dict(name=name, value=value, valid=error is None)
        if error:
            item['error'] = error
        else:
            item['state'] = dict(final_hash=value['final_hash'], pending_actions=pending or [],
                                 pending_inbox_wakes=wakes or [], pending_timers=[], pending_llm_requests=[])
        vectors.append(item)
        return value

    def trailing(sample, changes):
        before = sample['records'][-1]['output_files'] if sample['records'] else sample['initial_files']
        output = dict(before)
        for change in changes:
            if change['after'] is None:
                output.pop(change['path'], None)
            else:
                output[change['path']] = change['after']
        record = dict(version='0.1', trace_id=sample['trace_id'], coglet=coglet,
                      logical_time=20, previous=sample['records'][-1]['record_hash'] if sample['records'] else None,
                      before_hash=digest(before), output_hash=digest(output), output_files=output, boundary=changes)
        sample['trailing_boundary'] = seal(record)
        sample['final_hash'] = record['output_hash']
        return sample

    def write(path, before, after):
        return dict(actor=hub, audit_id='6'*32, ticket=None, method='manage.write',
                    target=coglet, path=path, before=before, after=after)

    standalone = clone(trace)
    standalone['records'] = []
    add('standalone-durable-write', trailing(standalone, [write('/data/count', '30', '31')]))
    add('standalone-ordered-write-delete', trailing(clone(standalone), [
        write('/data/count', '30', '31'), write('/data/count', '31', None)]))
    after_step = add('trailing-durable-write', trailing(clone(trace), [write('/data/count', '30', '31')]))
    aborted = clone(trace)
    aborted['records'][0].update(status='aborted', failure=dict(kind='guest_return',
        error=dict(code='internal', message='guest returned nonzero', retryable=False)))
    seal(aborted['records'][0])
    add('trailing-write-after-abort', trailing(aborted, [write('/data/count', '30', '31')]))
    add('zero-step-unchanged-snapshot', dict(clone(trace), records=[]))
    for name, change, error in [
        ('boundary-before-mismatch', write('/data/count', '32', '31'), 'boundary target or before'),
        ('boundary-code-write', write('/code/bin/step.wasm', base['input_files']['/code/bin/step.wasm'], '00'), 'boundary mount'),
        ('boundary-log-delete', write('/logs/stdout', None, None), 'boundary log rewrite'),
    ]:
        add(name, trailing(dict(clone(trace), records=[]), [change]), error)
    bad = clone(after_step)
    bad['trailing_boundary']['previous'] = '0'*64
    seal(bad['trailing_boundary'])
    add('trailing-wrong-previous', bad, 'boundary record chain')
    bad = clone(after_step)
    bad['trailing_boundary']['logical_time'] = 9
    seal(bad['trailing_boundary'])
    add('trailing-time-regression', bad, 'boundary time regressed')
    bad = clone(after_step)
    bad['trailing_boundary'] = None
    add('trailing-omitted-with-final-change', bad, 'final hash')
    bad = clone(after_step)
    bad['trailing_boundary']['output_files']['/data/hidden'] = '30'
    bad['trailing_boundary']['output_hash'] = digest(bad['trailing_boundary']['output_files'])
    seal(bad['trailing_boundary'])
    bad['final_hash'] = bad['trailing_boundary']['output_hash']
    add('trailing-unattributed-mutation', bad, 'boundary snapshot')

    def terminal(kind, payload=None, source=None):
        if payload is None:
            payload = (dict(id=action['id'], correlation_id=action['correlation_id'], accepted=False,
                            error=dict(code='permission_denied', message='grant revoked', retryable=False))
                       if kind == 'runtime.action_rejected' else
                       dict(id=action['id'], correlation_id=action['correlation_id'], status='succeeded', result={}))
        source = source or (coglet if kind == 'runtime.action_rejected' else surface)
        event = dict(source=source, kind=kind, seq=1, logical_time=20,
                     correlation_id=payload['correlation_id'], payload=payload)
        return dict(index=0, id='7'*32, sender=source, event=event)

    def append(sample, messages):
        raw = ''.join(json.dumps(m, sort_keys=True, separators=(',', ':'), ensure_ascii=False)+'\n' for m in messages).encode().hex()
        change = dict(actor=coglet, audit_id='8'*32, ticket=None, method='runtime.receipt',
                      target=coglet, path='/data/inbox.jsonl', before=None, after=raw)
        return trailing(sample, [change])

    pending = dict(clone(trace), records=[], initial_actions=[action],
                   initial_effect_ids=dict(actions=[action['id']], timers=[], llm_requests=[]))
    add('initial-action-pending-without-delivery', pending, pending=[action['id']])
    controls = {}
    for kind, label in [('runtime.action_rejected', 'rejection'), ('action.outcome', 'outcome')]:
        message = terminal(kind)
        sample = add('terminal-guest-'+label+'-standalone', append(clone(pending), [message]), wakes=[0])
        controls[label] = sample
        # A committed emit followed by terminal materialization, with no fake second step.
        emitted = clone(trace)
        emitted['records'][0]['calls'] = [dict(op='emit', argument=action, result=0)]
        emitted['records'][0]['actions'] = [action]
        seal(emitted['records'][0])
        add('terminal-guest-'+label+'-after-commit', append(emitted, [message]), wakes=[0])
        # Next trace starts after terminal append. Pending action is already consumed,
        # but the ordinary wake may still need delivery, even on a failed attempt.
        for abort in (False, True):
            delivered = clone(trace)
            rec = clone(base)
            rec.update(input_files=clone(sample['trailing_boundary']['output_files']),
                       output_files=clone(sample['trailing_boundary']['output_files']))
            rec.update(before_hash=digest(rec['input_files']), input_hash=digest(rec['input_files']), output_hash=digest(rec['output_files']))
            rec['events'] = [dict(source=coglet, kind='runtime.inbox', seq=2, logical_time=10,
                payload=dict(index=0, id=message['id']), correlation_id=action['correlation_id'])]
            if abort:
                rec.update(status='aborted', failure=dict(kind='guest_return', error=dict(
                    code='internal', message='failed guest attempt', retryable=False)))
            delivered.update(initial_files=clone(rec['input_files']), initial_hash=rec['before_hash'],
                final_hash=rec['output_hash'], initial_inbox_wakes=[0],
                initial_effect_ids=clone(pending['initial_effect_ids']), records=[seal(rec)])
            add('terminal-'+label+'-wake-'+('abort' if abort else 'delivery'), delivered)
        for mutation, error in [('unknown-id', 'terminal action'), ('correlation', 'terminal action'),
                                ('duplicate', 'terminal action'), ('source', 'terminal .*source'),
                                ('origin', 'terminal receipt origin')]:
            messages = [clone(message)]
            if mutation == 'unknown-id':
                messages[0]['event']['payload']['id'] = '9'*32
            elif mutation == 'correlation':
                messages[0]['event']['payload']['correlation_id'] = '9'*32
                messages[0]['event']['correlation_id'] = '9'*32
            elif mutation == 'duplicate':
                extra = clone(message)
                extra.update(index=1, id='9'*32)
                extra['event']['seq'] = 2
                messages.append(extra)
            elif mutation == 'source':
                messages[0]['sender'] = hub
                messages[0]['event']['source'] = hub
            bad = append(clone(pending), messages)
            if mutation == 'origin':
                bad['trailing_boundary']['boundary'][0]['method'] = 'hub.inbox'
                seal(bad['trailing_boundary'])
            add('terminal-'+label+'-'+mutation, bad, error)
    # Delivery before a later real guest attempt: the terminal is input, not
    # a result of that same attempt. Preserve pending-wake semantics on abort.
    for label, sample in controls.items():
        for abort in (False, True):
            joined = clone(trace)
            first = clone(base)
            first['calls'] = [dict(op='emit', argument=action, result=0)]
            first['actions'] = [action]
            seal(first)
            second = clone(base)
            second.update(step=1, logical_time=20, previous=first['record_hash'],
                boundary=clone(sample['trailing_boundary']['boundary']),
                input_files=clone(sample['trailing_boundary']['output_files']),
                output_files=clone(sample['trailing_boundary']['output_files']))
            second.update(input_hash=digest(second['input_files']), output_hash=digest(second['output_files']))
            second['events'] = [dict(source=coglet, kind='runtime.inbox', seq=2, logical_time=20,
                correlation_id=action['correlation_id'], payload=dict(index=0, id='7'*32))]
            if abort:
                second.update(status='aborted', failure=dict(kind='guest_return', error=dict(
                    code='internal', message='wake failed', retryable=False)))
            joined.update(records=[first, seal(second)], final_hash=second['output_hash'])
            add('terminal-'+label+'-prestep-'+('abort' if abort else 'delivery'), joined)
    for status in ('failed', 'cancelled', 'expired'):
        message = terminal('action.outcome', dict(id=action['id'], correlation_id=action['correlation_id'],
            status=status, error=dict(code='cancelled' if status == 'cancelled' else
                'deadline_exceeded' if status == 'expired' else 'internal',
                message='terminal failure', retryable=False)))
        add('terminal-outcome-'+status, append(clone(pending), [message]), wakes=[0])
    add('terminal-runtime-synthesized-outcome', append(clone(pending),
        [terminal('action.outcome', source=coglet)]), wakes=[0])
    # Rejection forbids positive admission and missing common error fields.
    for accepted in (True, False):
        message = terminal('runtime.action_rejected')
        message['event']['payload']['accepted'] = accepted
        del message['event']['payload']['error']
        add('terminal-rejection-'+('accepted-true' if accepted else 'missing-error'),
            append(clone(pending), [message]), 'embedded action-rejection')
    bad = clone(controls['rejection'])
    bad['initial_effect_ids']['actions'] = []
    add('pending-action-missing-ledger', bad, 'initial pending work')
    bad = clone(controls['outcome'])
    bad['initial_actions'].append(clone(action))
    add('pending-action-duplicate', bad, 'initial pending work')
    # An abort never installs its staged emit into pending state.
    bad = clone(trace)
    rec = bad['records'][0]
    rec.update(status='aborted', calls=[dict(op='emit', argument=action, result=0)],
        failure=dict(kind='guest_return', error=dict(code='internal', message='abort', retryable=False)))
    seal(rec)
    add('terminal-after-aborted-emit', append(bad, [terminal('action.outcome')]), 'terminal action')
    for label, sample in controls.items():
        bad = clone(sample)
        bad.update(initial_files=clone(bad['trailing_boundary']['output_files']),
                   initial_hash=bad['final_hash'], trailing_boundary=None)
        add('initial-'+label+'-already-terminal', bad, 'initial action already terminal')
    path = SPEC / 'fixtures/abi/boundary-receipts.fixture'
    if write_files:
        path.write_text(json.dumps(vectors, indent=2)+'\n')
    timelines = []
    for vector in vectors:
        result = dict(valid=vector['valid'], dispatched=False, engine_invocations=0)
        if vector['valid']:
            result['state'] = vector['state']
        timelines.append(dict(name=vector['name'], steps=[
            dict(send=dict(operation='abi.validate_trace', trace=vector['value'])),
            dict(expect=result, within_ms=200)]))
    if write_files:
        (SPEC / 'fixtures/abi/boundary-receipts-timeline.fixture').write_text(json.dumps(timelines, indent=2)+'\n')
    return vectors, timelines


if __name__ == '__main__':
    print(f'generated {len(generate()[0])} M-A boundary/terminal case contracts')
