"""Fail-closed, explicit codec fixture discovery shared by Python consumers."""
import json

MARKER = 'semantic-vectors-v1'


def cases(path):
    marker = path.with_name(path.name + '.format')
    if marker.exists():
        text = marker.read_text()
        if text not in (MARKER, MARKER + '\n'):
            raise ValueError(f'{marker}: unknown fixture format {text!r}')
        # A marker does not make malformed JSON acceptable.
        json.loads(path.read_text())
        return None
    data = json.loads(path.read_text())
    if not isinstance(data, list) or not data:
        raise ValueError(f'{path}: expected nonempty codec case array or explicit .format marker')
    names = set()
    for case in data:
        if not isinstance(case, dict) or not isinstance(case.get('name'), str) or case['name'] in names:
            raise ValueError(f'{path}: invalid or duplicate case name')
        names.add(case['name'])
        if 'valid' in case:
            if type(case['valid']) is not bool or not isinstance(case.get('type'), str) or 'value' not in case:
                raise ValueError(f'{path}:{case["name"]}: malformed shape case')
            if 'semantic' in case and type(case['semantic']) is not bool:
                raise ValueError(f'{path}:{case["name"]}: semantic must be boolean')
            if case['valid'] and not isinstance(case.get('cbor_hex'), str):
                raise ValueError(f'{path}:{case["name"]}: golden missing cbor_hex')
        elif not isinstance(case.get('cbor_hex'), str) or 'type' in case or 'semantic' in case:
            raise ValueError(f'{path}:{case["name"]}: malformed wire negative')
    return data


def shape_valid(case):
    return case['valid'] or case.get('semantic', False)
