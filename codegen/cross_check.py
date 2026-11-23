"""Cross-check generated CDDL validation against independently executed schemas."""
import json
from pathlib import Path
import sys
import subprocess
import tempfile

import cbor2
from jsonschema import Draft202012Validator
from referencing import Registry, Resource
from pycddl import Schema, ValidationError
from fixtures import cases as load_cases, shape_valid

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'sdk/surface-python'))
from coglet_surface.codec import validate


def main():
    cddl = '\n'.join(p.read_text() for p in sorted((ROOT / 'spec/messages').glob('*.cddl')))
    schemas = [json.loads(p.read_text()) for p in sorted((ROOT / 'spec/messages').glob('*.schema.json'))]
    registry = Registry().with_resources((s['$id'], Resource.from_contents(s)) for s in schemas)
    definitions = {}
    for schema in schemas:
        Draft202012Validator.check_schema(schema)
        for name in schema.get('$defs', {}):
            if name in definitions:
                raise ValueError('ambiguous JSON Schema definition: ' + name)
            definitions[name] = schema
    count = skipped = semantic = ruby_checks = 0
    for path in sorted((ROOT / 'spec/fixtures').rglob('*.json')):
        cases = load_cases(path)
        if cases is None:
            skipped += 1
            continue
        for case in cases:
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
            # pycddl 0.6.4 rejects some valid nested-map/regexp alternatives.
            # A second real CDDL implementation must decide, never a waiver or
            # a comparison to the generated validator itself. Surface owns the
            # version-locked Ruby validator and installs it in its CI snapshot.
            if cd != shape_valid(case) and (ROOT / 'spec/tests/cddl_check.rb').exists():
                with tempfile.NamedTemporaryFile(mode='w', suffix='.cddl') as source:
                    source.write(cddl)
                    source.flush()
                    result = subprocess.run(['ruby', str(ROOT / 'spec/tests/cddl_check.rb'), rule, source.name],
                                            input=json.dumps(case['value']), text=True, capture_output=True)
                if result.returncode not in (0, 1):
                    raise ValueError(f'{path}:{case["name"]}: Ruby CDDL failed: {result.stderr}')
                cd = result.returncode == 0
                ruby_checks += 1
            try:
                validate(rule, case['value'])
                generated = True
            except ValueError:
                generated = False
            semantic += bool(case.get('semantic'))
            if (js, cd, generated) != (shape_valid(case),) * 3:
                raise AssertionError(f'{path}:{case["name"]}: JSON={js}, CDDL={cd}, generated={generated}')
            count += 1
    if not count:
        raise AssertionError('no semantic fixtures found')
    print(f'cross-check: {count} fixtures agree across CDDL, JSON Schema and generated validator; {semantic} protocol-semantic, {skipped} semantic-vectors-v1 files skipped, {ruby_checks} independent Ruby CDDL fallbacks')


if __name__ == '__main__':
    main()
