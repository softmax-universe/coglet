"""Generate separate 0.2 naming vectors; never edit frozen 0.1 cases."""
import argparse
import copy
import json
from pathlib import Path
from test_common import encode

ROOT = Path(__file__).resolve().parents[2]
COG = 'coglet:' + 'b' * 64
CID = '1' * 32
SID = 'surface:' + 'a' * 64


def build():
    cases = []

    def add(name, rule, value, valid):
        case = dict(name=name, type=rule, value=value, valid=valid)
        if valid:
            case['cbor_hex'] = encode(value).hex()
        cases.append(case)

    names = ['speech.start', 'softmax.app', 'softmax.app_sensitive',
             'softmax.app.state', 'softmax.app.a' + '0' * 31]
    invalid = ['softmax.app.', 'softmax.app.a' + '0' * 32, 'softmax.app.a.b',
               'other.app.state', 'softmax.apple.state', 'softmax.app.State',
               'softmax.app.state\n', 'softmax.app.state\r', 'softmax.app.state\u0000',
               'softmax.app.é', 'softmax..state', 'softmax.app.0state', '']
    for i, name in enumerate(names + invalid):
        add(f'name-{i}', 's02-capability-name', name, name in names)
        add(f'frozen-name-{i}', 'capability-name', name,
            name in ('speech.start', 'softmax.app', 'softmax.app_sensitive'))

    event = dict(source=SID, kind='softmax.app.view_changed', seq=0,
                 logical_time=0, payload={}, correlation_id=CID)
    action = dict(id=CID, coglet=COG, capability='softmax.app.navigate', args={},
                  deadline=1000, correlation_id=CID)
    entry = dict(name='softmax.app', events=['softmax.app.view_changed'],
                 reads=['softmax.app.state'], actions=['softmax.app.navigate'],
                 exclusive=['softmax.app_control'], schemas={'softmax.app.state': 'https://example.org/state'})
    records = {
        'event': event,
        'action': action,
        'capabilities': dict(revision=1, clock=0, entries=[entry], correlation_id=CID),
        'grants': dict(epoch=1, entries=[dict(coglet=COG, operations=['softmax.app.navigate'],
                                             resources=['softmax.app_control'], expires=1000)], correlation_id=CID),
        'read': dict(coglet=COG, capability='softmax.app.state', args={}, grant_epoch=1,
                     deadline=1000, correlation_id=CID),
        'act': dict(grant_epoch=1, action=action),
    }
    budgets = dict(compute=1, compute_mode='fuel', memory_pages=1, wall_time_us=1,
                   max_events=1, host_bytes=1)
    manifest = dict(version='0.1', id=COG, subscriptions=['softmax.app.view_changed'],
                    required_capabilities=['softmax.app'], exclusive_outputs=['softmax.app_control'], budgets=budgets)
    records['manifest'] = manifest
    records['db-manifest'] = dict(manifest, version='0.2',
                                  database=dict(schema_min=0, schema_max=1, catalog_hash='0' * 64))
    for rule, record in records.items():
        add(rule + '-app-opt-in', 's02-' + rule, record, True)
        add(rule + '-app-frozen-refused', rule, record, False)
        extra = dict(record, unexpected=True)
        add(rule + '-closed', 's02-' + rule, extra, False)
        # Old two-component names remain shape-valid under the opt-in rules.
        def rename(v):
            if isinstance(v, str):
                return v.replace('softmax.app.', 'speech.')
            if isinstance(v, list):
                return [rename(x) for x in v]
            if isinstance(v, dict):
                return {rename(k): rename(x) for k, x in v.items()}
            return v
        add(rule + '-two-component-compatible', 's02-' + rule, rename(record), True)

    for rule in ('manifest', 'db-manifest'):
        wrong = copy.deepcopy(records[rule])
        wrong['version'] = '0.2' if rule == 'manifest' else '0.1'
        add(rule + '-format-not-profile', 's02-' + rule, wrong, False)
    for field in ('subscriptions', 'required_capabilities', 'exclusive_outputs'):
        wrong = copy.deepcopy(manifest)
        wrong[field] = ['other.app.state']
        add('manifest-' + field + '-namespace-refused', 's02-manifest', wrong, False)
    for wire, rule in [('event', 'event'), ('capabilities', 'capabilities'),
                       ('grants', 'grants'), ('read', 'read'), ('act', 'act')]:
        add('wire-' + wire, 's02-surface-message', dict(type=wire, body=records[rule]), True)
    return json.dumps(cases, indent=2, ensure_ascii=False) + '\n'


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    path = ROOT / 'spec/fixtures/surface02/cases.json'
    content = build()
    if args.check:
        if not path.exists() or path.read_text() != content:
            raise SystemExit('stale surface02 fixtures')
    else:
        path.parent.mkdir(exist_ok=True)
        path.write_text(content)


if __name__ == '__main__':
    main()
