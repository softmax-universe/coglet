#!/usr/bin/env python3
"""Standalone product generation; does not invoke or mutate the shared registry."""
import argparse
import hashlib
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(ROOT / 'codegen'))
from cddl import parse
from swift import generate_swift

BASE = 'https://softmax.com/schemas/app/v1/'
VOCAB = 'https://softmax.com/schemas/vocab/utf8-byte-length/v1'
# Exact operation binding selection, never a root union.
BINDINGS = {
    'state': ('read', 'app-empty', 'app-state-result'),
    'controls': ('read', 'app-controls-args', 'app-controls-result'),
    'view_changed': ('event', 'app-view-changed', None),
    'focus_changed': ('event', 'app-focus-changed', None),
    'control_changed': ('event', 'app-control-changed', None),
    'window_changed': ('event', 'app-window-changed', None),
    'navigate': ('action', 'app-location', 'app-empty'),
    'open_window': ('action', 'app-window-args', 'app-empty'),
    'close_window': ('action', 'app-window-args', 'app-empty'),
    'set_focus': ('action', 'app-focus-args', 'app-empty'),
    'invoke': ('action', 'app-invoke-args', 'app-empty'),
    'set_setting': ('action', 'app-setting-args', 'app-empty'),
    'invoke_sensitive': ('action', 'app-invoke-args', 'app-empty'),
}


def json_node(n):
    """Translate the closed subset exactly; UTF-8 size is a required vocabulary.

    JSON Schema's maxLength counts Unicode codepoints, NOT UTF-8 bytes. Never
    silently substitute it for CDDL .size. See schema_validation.py.
    """
    op = n['op']
    if op == 'ref': return {'$ref': '#/$defs/' + n['name']}
    if op == 'literal': return {'const': n['value']}
    if op == 'primitive': return {'type': {'tstr': 'string', 'bool': 'boolean', 'null': 'null'}[n['name']]}
    if op == 'range': return {'type': 'integer', 'minimum': n['min'], 'maximum': n['max']}
    if op == 'size': return {'type': 'string', 'minUtf8Bytes': n['min'], 'maxUtf8Bytes': n['max']}
    if op in ('union', 'intersection'): return {('anyOf' if op == 'union' else 'allOf'): [json_node(x) for x in n['items']]}
    if op == 'array':
        return {'type': 'array', 'items': json_node(n['item']), 'minItems': n['min'], 'maxItems': n['max']}
    if op == 'record':
        assert set(n) == {'op', 'fields'}, 'open records forbidden'
        return {'type': 'object', 'properties': {f['name']: json_node(f['type']) for f in n['fields']},
                'required': [f['name'] for f in n['fields'] if not f['optional']], 'additionalProperties': False}
    raise ValueError('unsupported product node: ' + op)


def dump(value):
    return (json.dumps(value, indent=2, ensure_ascii=False, sort_keys=True) + '\n').encode()


def artifacts():
    rules = parse((HERE / 'softmax-app.cddl').read_text())
    operations = {}
    schemas = {}
    for name, (direction, inp, out) in BINDINGS.items():
        sensitive = name == 'invoke_sensitive'
        entry = 'softmax.app_sensitive' if sensitive else 'softmax.app'
        record = {'entry': entry, 'direction': direction, 'input': BASE + inp,
                  'resources': [('softmax.app_sensitive_control' if sensitive else 'softmax.app_control')] if direction == 'action' else [],
                  'single_owner': True}
        schemas[BASE + inp] = inp
        if out:
            record['output'] = BASE + out
            schemas[BASE + out] = out
        operations['softmax.app.' + name] = record
    catalog = {'version': 1, 'namespace': 'softmax',
               'entries': {'softmax.app': ['softmax.app_control'], 'softmax.app_sensitive': ['softmax.app_sensitive_control']},
               'schemas': schemas, 'rules': rules, 'operations': operations}
    raw = dump(catalog)
    if len(raw) > 1_048_576: raise ValueError('catalog exceeds 1 MiB')
    schema = {'$schema': 'https://json-schema.org/draft/2020-12/schema',
              '$id': BASE + 'schema.json',
              '$comment': 'Requires the mandatory UTF-8 byte-length vocabulary; use schema_validation.validator. No root payload union.',
              '$vocabulary': {'https://json-schema.org/draft/2020-12/vocab/core': True,
                              'https://json-schema.org/draft/2020-12/vocab/applicator': True,
                              'https://json-schema.org/draft/2020-12/vocab/validation': True, VOCAB: True},
              '$defs': {k: json_node(v) for k, v in rules.items()}}
    # The existing emitter and dependency-free codec are compiled together in a
    # standalone product module. SchemaNode is intentionally SDK-internal; merely
    # importing CogletTypes would silently select built-in IR or fail compilation.
    swift = generate_swift(rules)
    from product_fixtures import cases
    import cbor2
    corpus = cases(rules)
    for case in corpus:
        case['cbor_hex'] = cbor2.dumps(case['value'], canonical=True).hex()
    return {'fixtures.json': dump(corpus), 'softmax-app.schema.json': dump(schema), 'softmax-app.ir.json': dump(rules),
            'catalog.v1.json': raw, 'catalog.v1.sha256': (hashlib.sha256(raw).hexdigest() + '\n').encode(),
            'Sources/SoftmaxAppSurface/CBOR.swift': (ROOT / 'sdk/surface-swift/Sources/CogletTypes/CBOR.swift').read_bytes(),
            'Sources/SoftmaxAppSurface/Generated/Types.swift': swift.encode()}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    stale = []
    for name, raw in artifacts().items():
        path = HERE / name
        if args.check:
            if not path.exists() or path.read_bytes() != raw: stale.append(name)
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(raw)
    if stale: raise SystemExit('stale product artifacts: ' + ', '.join(stale))
    print(('checked' if args.check else 'generated') + ' 7 product artifacts; 13 operation bindings')

if __name__ == '__main__': main()
