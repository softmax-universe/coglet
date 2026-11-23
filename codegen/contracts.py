"""Explicit, exhaustive classification of schema files and fixture families."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def load(root=ROOT):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f'duplicate contract: {key}')
            result[key] = value
        return result

    contracts = json.loads((root / 'codegen/contracts.json').read_text(), object_pairs_hook=unique)
    if not isinstance(contracts, dict) or not contracts:
        raise ValueError('contracts must be a nonempty object')
    if any(mode not in ('generated', 'draft-validated') for mode in contracts.values()):
        raise ValueError('unknown contract classification')
    paths = [*(root / 'spec/messages').glob('*.cddl'), *(root / 'spec').glob('*.cddl')]
    schemas = {p.stem for p in paths}
    if len(schemas) != len(paths):
        raise ValueError('ambiguous CDDL contract stem')
    fixture_root = root / 'spec/fixtures'
    families = {p.name for p in fixture_root.iterdir() if p.is_dir()}
    for label, discovered in [('schemas', schemas), ('fixtures', families)]:
        if discovered != set(contracts):
            raise ValueError(f'unclassified or stale {label}: {sorted(discovered ^ set(contracts))}')
    if list(fixture_root.glob('*.json')):
        raise ValueError('unclassified root fixture files')
    for name in contracts:
        if not list((fixture_root / name).rglob('*.json')):
            raise ValueError(f'empty fixture family: {name}')
    return contracts


def schema_paths(root=ROOT):
    contracts = load(root)
    return sorted(p for directory in (root / 'spec', root / 'spec/messages')
                  for p in directory.glob('*.cddl') if contracts[p.stem] == 'generated')


def fixture_paths(root=ROOT):
    return sorted(p for name, mode in load(root).items() if mode == 'generated'
                  for p in (root / 'spec/fixtures' / name).rglob('*.json'))
