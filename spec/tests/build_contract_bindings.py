"""M-B binding catalog and shape-valid/invalid selected-rule fixtures.

No runtime, authority or real provider qualification is implemented here.
"""
import copy
import json
import sys
from pathlib import Path

import build_surface_fixtures as build
from test_common import encode

ROOT = Path(__file__).resolve().parents[2]
# Each tuple is operation, entry, direction, input/payload rule, output rule.
TABLE = [
    ('speech.start', 'speech.control', 'event', 'speech:speech-start', None),
    ('speech.partial', 'speech.control', 'event', 'speech:speech-partial', None),
    ('speech.end', 'speech.control', 'event', 'speech:speech-end', None),
    ('speech.position', 'speech.control', 'event', 'speech:speech-position', None),
    ('speech.barge_in', 'speech.control', 'event', 'speech:speech-barge-in', None),
    ('speech.config', 'speech.control', 'read', 'surface:empty-map', 'speech:speech-config'),
    ('speech.speak_chunk', 'speech.control', 'action', 'speech:speak-chunk', 'surface:empty-map'),
    ('speech.stop', 'speech.control', 'action', 'speech:stop', 'surface:empty-map'),
    ('speech.backchannel', 'speech.control', 'action', 'speech:backchannel', 'surface:empty-map'),
    ('speech.session_open', 'speech.session', 'action', 'speech:session-open', 'surface:empty-map'),
    ('speech.session_close', 'speech.session', 'action', 'speech:session-close', 'surface:empty-map'),
    ('speech.tool_call', 'speech.session', 'event', 'speech:tool-call', None),
    ('speech.tool_result', 'speech.session', 'action', 'speech:tool-result', 'surface:empty-map'),
    ('speech.heard', 'speech.history', 'read', 'speech:heard-query', 'speech:heard-record'),
    ('speech.work_state', 'speech.work', 'read', 'surface:empty-map', 'speech:work-state'),
    ('speech.work_intent', 'speech.work', 'event', 'speech:work-intent', None),
    ('speech.work_done', 'speech.work', 'event', 'speech:work-done', None),
    ('speech.work_result', 'speech.work', 'action', 'speech:work-result', 'surface:empty-map'),
    ('expression.set_expression', 'expression.control', 'action', 'expression:set-expression', 'surface:empty-map'),
    ('expression.state', 'expression.control', 'read', 'surface:empty-map', 'expression:expression-state'),
    ('sim.gridbot_state', 'sim.gridbot', 'event', 'sandbox:gridbot-state', None),
    ('sim.gridbot_move', 'sim.gridbot', 'action', 'sandbox:gridbot-move', 'surface:empty-map'),
    ('sim.cartpole_state', 'sim.cartpole', 'event', 'sandbox:cartpole-state', None),
    ('sim.cartpole_force', 'sim.cartpole', 'action', 'sandbox:cartpole-force', 'surface:empty-map'),
    ('llm.complete', None, 'gateway', 'abi:llm-request', 'abi:llm-reply'),
]


def catalog():
    return {'version': '0.1', 'operations': {
        op: dict(entry=entry, direction=direction, input=arg, output=result)
        for op, entry, direction, arg, result in TABLE}}


def examples():
    values = {f'{family}:{rule}': value for family, rules in build.values.items()
              for rule, value in rules.items()}
    from build_sandbox import VALUES
    values.update({'sandbox:' + rule: value for rule, value in VALUES.items()})
    values['surface:empty-map'] = {}
    abi = json.loads((ROOT / 'spec/fixtures/abi/cases.json').read_text())
    for rule in ('llm-request', 'llm-reply'):
        values['abi:' + rule] = next(c['value'] for c in abi if c['type'] == rule and c['valid'])
    return values


def vectors():
    values = examples()
    result = []
    for op, binding in catalog()['operations'].items():
        for slot in ('input', 'output'):
            rule = binding[slot]
            if rule is None:
                continue
            good = copy.deepcopy(values[rule])
            candidates = [('valid', good, True), ('unknown-field', dict(good, unexpected=True), False)]
            if good:
                bad = copy.deepcopy(good)
                del bad[next(iter(bad))]
                candidates.append(('missing-field', bad, False))
            # A valid sibling payload is not a valid selected operation payload.
            sibling = values['speech:speech-start'] if rule != 'speech:speech-start' else values['speech:stop']
            candidates.append(('wrong-operation', sibling, False))
            for suffix, value, valid in candidates:
                result.append(dict(name=f'{op}-{slot}-{suffix}', operation=op, slot=slot,
                                   valid=valid, value=value,
                                   **({'cbor_hex': encode(value).hex()} if valid else {})))
    return result


def outputs():
    return {'spec/capabilities/bindings.json': build.dumps(catalog()),
            'spec/fixtures/surface/bindings.fixture': build.dumps(vectors())}


def main():
    for path, text in outputs().items():
        dest = ROOT / path
        if '--check' in sys.argv:
            if not dest.exists() or dest.read_text() != text:
                raise SystemExit('Stale binding file: ' + path)
        else:
            dest.write_text(text)


if __name__ == '__main__':
    main()
