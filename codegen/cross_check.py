"""Cross-check generated CDDL validation against independently executed schemas."""
import json
from pathlib import Path
import sys
from contracts import schema_paths, fixture_paths

import cbor2
from jsonschema import Draft202012Validator
from referencing import Registry, Resource
from pycddl import Schema, ValidationError

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'sdk/surface-python'))
from coglet_surface.codec import validate


def main():
    paths = schema_paths(ROOT)
    cddl = '\n'.join(p.read_text() for p in paths)
    schemas = [json.loads(p.with_suffix('.schema.json').read_text()) for p in paths]
    registry = Registry().with_resources((s['$id'], Resource.from_contents(s)) for s in schemas)
    definitions = {}
    for schema in schemas:
        Draft202012Validator.check_schema(schema)
        for name in schema.get('$defs', {}):
            if name in definitions:
                raise ValueError('ambiguous JSON Schema definition: ' + name)
            definitions[name] = schema
    count = 0
    for path in fixture_paths(ROOT):
        for case in json.loads(path.read_text()):
            if 'valid' not in case:
                continue
            rule = case['type']
            schema = definitions[rule]
            selected = {'$schema': schema['$schema'], '$ref': schema['$id'] + '#/$defs/' + rule}
            js = Draft202012Validator(selected, registry=registry).is_valid(case['value'])
            try:
                Schema(f'fixture = {rule}\n' + cddl).validate_cbor(cbor2.dumps(case['value']))
                cd = True
            except ValidationError:
                cd = False
            try:
                validate(rule, case['value'])
                generated = True
            except ValueError:
                generated = False
            if (js, cd, generated) != (case['valid'],) * 3:
                raise AssertionError(f'{path}:{case["name"]}: JSON={js}, CDDL={cd}, generated={generated}')
            count += 1
    if not count:
        raise AssertionError('no semantic fixtures found')
    print(f'cross-check: {count} fixtures agree across CDDL, JSON Schema and generated validator')


if __name__ == '__main__':
    main()
