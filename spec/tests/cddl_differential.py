"""Recursive CDDL/JSON Schema differential audit, including public roots.

Usage: python spec/tests/cddl_differential.py --spec SPEC_DIR --family common
Run against spec-owner snapshots before freeze; fixture validation is not engine
execution. Every value at every depth is replaced; every map gets an unknown key.
A JSON Schema verdict is a comparison, never an input to the CDDL backend.
"""
import argparse
import copy
import json
from pathlib import Path
import re

import cbor2
from jsonschema import Draft202012Validator
from referencing import Registry, Resource

from reference_cddl import Schema, ValidationError

REPLACEMENTS = (0, 'wrong', b'wrong', True, None, [], [1], [[]], {'x': 1})


def mutations(value):
    """Yield paths and replacements, preserving every other sibling."""
    yield (), value  # unchanged positive control, including legitimate arrays
    def walk(node, path):
        for replacement in REPLACEMENTS:
            yield path, replacement
        if isinstance(node, dict):
            key = '__unknown_reference_field__'
            while key in node:
                key += '_'
            yield path, dict(node, **{key: 1})
            for key, child in node.items():
                yield from walk(child, path + (key,))
        elif isinstance(node, list):
            for index, child in enumerate(node):
                yield from walk(child, path + (index,))
    for path, replacement in walk(value, ()):
        result = copy.deepcopy(value)
        if not path:
            result = replacement
        else:
            parent = result
            for part in path[:-1]:
                parent = parent[part]
            parent[path[-1]] = replacement
        yield path, result


def audit(spec, family, extra_roots=(), includes=()):
    spec = Path(spec)
    family_cddl = spec / 'messages' / (family + '.cddl')
    if not family_cddl.exists():
        family_cddl = spec / (family + '.cddl')
    paths = [family_cddl]
    for name in includes:
        path = spec / 'messages' / (name + '.cddl')
        if not path.exists():
            path = spec / (name + '.cddl')
        paths.append(path)
    if family != 'common':
        paths.append(spec / 'messages/common.cddl')
    source = '\n'.join(path.read_text() for path in dict.fromkeys(paths))
    # The first assignment is the actual CDDL public entry, not a helper.
    public = re.search(r'^\s*([\w-]+)\s*=', re.sub(r';[^\n]*', '', source), re.M).group(1)
    documents = [json.loads(p.read_text()) for p in (spec / 'messages').glob('*.schema.json')]
    documents += [json.loads(p.read_text()) for p in spec.glob('*.schema.json')]
    registry = Registry().with_resources((s['$id'], Resource.from_contents(s)) for s in documents if '$id' in s)
    family_schema = spec / 'messages' / (family + '.schema.json')
    if not family_schema.exists():
        family_schema = spec / (family + '.schema.json')
    document = json.loads(family_schema.read_text())
    definitions = {}
    for d in documents:
        for name in d.get('$defs', {}):
            # The owning family wins over imports; no generated definitions.
            if name not in definitions or d.get('$id') == document.get('$id'):
                definitions[name] = d
    roots = [('$root', Schema(source), Draft202012Validator(document, registry=registry))]
    for name in extra_roots:
        d = definitions[name]
        roots.append((name, Schema('selected = ' + name + '\n' + source),
                      Draft202012Validator({'$ref': d['$id'] + '#/$defs/' + name}, registry=registry)))
    cases = []
    vector_files = wire_cases = 0
    for path in sorted((spec / 'fixtures' / family).rglob('*.json')):
        marker = path.with_name(path.name + '.format')
        if marker.exists():
            if marker.read_text() not in ('semantic-vectors-v1', 'semantic-vectors-v1\n'):
                raise ValueError(f'{marker}: unknown vector classification')
            json.loads(path.read_text())
            vector_files += 1
            continue  # counted non-codec; the protocol owner executes these
        data = json.loads(path.read_text())
        if not isinstance(data, list):
            raise ValueError(f'{path}: unclassified non-case JSON')
        for case in data:
            if 'valid' in case:
                if type(case['valid']) is not bool or not isinstance(case.get('type'), str) or 'value' not in case:
                    raise ValueError(f'{path}: malformed shape fixture')
                cases.append(case)
            elif 'cbor_hex' in case and 'type' not in case:
                wire_cases += 1
            else:
                raise ValueError(f'{path}: unclassified fixture')
    if not cases:
        raise ValueError(f'{family}: no shape fixtures')
    validators = {}
    seen = set()
    comparisons = controls = 0
    mismatches = []
    for case in cases:
        name = case['type']
        if name not in validators:
            d = definitions.get(name)
            selected = {'$ref': d['$id'] + '#/$defs/' + name} if d else document
            validators[name] = (name, Schema('selected = ' + name + '\n' + source),
                                Draft202012Validator(selected, registry=registry))
        # Negative fixtures are checked unchanged; only shape-positive cases
        # generate recursive substitutions. Protocol-semantic negatives may
        # still be shape-positive, per the shared fixture contract.
        values = mutations(case['value']) if case['valid'] or case.get('semantic', False) else [((), case['value'])]
        for path, value in values:
            raw = cbor2.dumps(value)
            for scope, cd, js in [validators[name], *roots]:
                key = scope, raw
                if key in seen:
                    continue
                seen.add(key)
                expected = js.is_valid(value)
                try:
                    cd.validate_cbor(raw)
                    actual = True
                except ValidationError:
                    actual = False
                comparisons += 1
                controls += expected
                if actual != expected:
                    mismatches.append({'case': case['name'], 'path': list(path), 'scope': scope,
                                       'json': expected, 'cddl': actual, 'cbor_hex': raw.hex()})
    result = {'family': family, 'public_cddl': public, 'fixtures': len(cases),
              'non_codec_vector_files': vector_files, 'wire_cases': wire_cases,
              'comparisons': comparisons, 'accepted_controls': controls,
              'mismatches': len(mismatches), 'findings': mismatches}
    print(json.dumps({k: v for k, v in result.items() if k != 'findings'}), flush=True)
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--spec', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--family', required=True)
    parser.add_argument('--root', action='append', default=[])
    parser.add_argument('--include', action='append', default=[], help='Additional imported CDDL family')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    result = audit(args.spec, args.family, args.root, args.include)
    if args.output:
        args.output.write_text(json.dumps(result, indent=2) + '\n')
    raise SystemExit(bool(result['mismatches']))
