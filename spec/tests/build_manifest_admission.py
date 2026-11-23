"""Generate data-only catalog/grant conformance cases; no runtime claim."""
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]


def cases():
    vectors = json.loads((ROOT / 'spec/fixtures/abi/manifest-admission.fixture').read_text())
    return [
        {'name': 'manifest-' + case['name'], 'steps': [
            {'send': {'operation': 'abi.manifest_admit',
                      'manifest': dict(vectors['manifest'], **case['manifest']),
                      'catalog': vectors['entries'],
                      'grant': dict(vectors['grant'], **case['grant'])}},
            {'expect': {'admitted': case['error'] is None,
                        'reason': case['error'], 'instantiated': False},
             'within_ms': 200},
        ]}
        for case in vectors['cases']
    ]


if __name__ == '__main__':
    path = ROOT / 'conformance/abi/manifest-admission.json'
    text = json.dumps(cases(), indent=2) + '\n'
    if '--check' in sys.argv:
        if not path.exists() or path.read_text() != text:
            raise SystemExit('manifest-admission.json is stale')
    else:
        path.write_text(text)
