"""Reproducible draft peering shape vectors, not authenticated execution."""
import copy
import json
from pathlib import Path
from test_common import encode

ROOT = Path(__file__).resolve().parents[2]
COMMON = 'common.schema.json#/$defs/'


def outputs():
    defs = {}
    ref = lambda name: {'$ref': COMMON + name}
    local = lambda name: {'$ref': '#/$defs/' + name}
    def record(name, fields, optional=()):
        defs[name] = {'type': 'object', 'properties': fields,
                      'required': [k for k in fields if k not in optional],
                      'additionalProperties': False}
    record('peer-enroll', dict(method={'const': 'peer.enroll'},
           correlation_id=ref('correlation-id'), household=ref('correlation-id'),
           sender=ref('hub-id'), receiver=ref('hub-id'), attending=ref('hub-id'),
           offer=ref('version-offer')))
    record('peer-enrolled', dict(method={'const': 'peer.enrolled'},
           correlation_id=ref('correlation-id'), selection=ref('version-selection')))
    record('peer-private', dict(source=ref('source-id'), owner=ref('coglet-id'), epoch=ref('uint64')))
    record('peer-route', dict(method={'const': 'peer.route'},
           correlation_id=ref('correlation-id'), household=ref('correlation-id'),
           sender=ref('hub-id'), receiver=ref('hub-id'),
           origin=ref('coglet-id'), target=ref('coglet-id'), actor=ref('node-id'),
           ticket=ref('correlation-id'), capability=ref('capability-name'),
           deadline=ref('uint64'), payload=ref('profile-map'), private=local('peer-private')),
           optional=('ticket', 'private'))
    record('peer-accepted', dict(method={'const': 'peer.accepted'}, correlation_id=ref('correlation-id')))
    record('peer-failure', dict(method={'const': 'peer.failure'}, correlation_id=ref('correlation-id'), error=ref('error')))
    defs['peering'] = {'oneOf': [local(n) for n in ('peer-enroll', 'peer-enrolled', 'peer-route', 'peer-accepted', 'peer-failure')]}
    schema = {'$schema': 'https://json-schema.org/draft/2020-12/schema',
              '$id': 'https://coglet.dev/spec/messages/peering.schema.json',
              '$ref': '#/$defs/peering', '$defs': defs}
    hub = lambda c: 'hub:' + c * 64
    coglet = lambda c: 'coglet:' + c * 64
    values = {
        'peer-enroll': dict(method='peer.enroll', correlation_id='1'*32, household='2'*32,
                            sender=hub('a'), receiver=hub('b'), attending=hub('a'), offer={'versions': ['0.1']}),
        'peer-enrolled': dict(method='peer.enrolled', correlation_id='1'*32, selection={'version': '0.1'}),
        'peer-private': dict(source='surface:'+'e'*64, owner=coglet('d'), epoch=1),
        'peer-route': dict(method='peer.route', correlation_id='3'*32, household='2'*32,
                           sender=hub('a'), receiver=hub('b'), origin=coglet('c'), target=coglet('d'),
                           actor='f'*64, capability='home.query', deadline=1000, payload={}),
        'peer-accepted': dict(method='peer.accepted', correlation_id='3'*32),
        'peer-failure': dict(method='peer.failure', correlation_id='3'*32,
                             error=dict(code='permission_denied', message='Unavailable', retryable=False)),
    }
    cases = []
    def add(name, rule, value, valid):
        case = dict(name=name, type=rule, value=value, valid=valid)
        if valid:
            case['cbor_hex'] = encode(value).hex()
        cases.append(case)
    for name, value in values.items():
        add(name, name, value, True)
        bad = dict(value, unknown=0)
        add(name+'-unknown', name, bad, False)
        for field in value:
            bad = copy.deepcopy(value)
            del bad[field]
            add(name+'-missing-'+field, name, bad, False)
    private = dict(values['peer-route'], private=values['peer-private'], ticket='4'*32)
    add('private-route', 'peer-route', private, True)
    for field, bad_value in [('sender', hub('a')+'\n'), ('actor', 'not-a-key'),
                             ('deadline', -1), ('private', {}), ('ticket', None)]:
        add('route-invalid-'+field, 'peer-route', dict(private, **{field: bad_value}), False)
    add('root-route', 'peering', private, True)
    add('root-unknown-method', 'peering', dict(private, method='hub.inbox'), False)
    return {'spec/messages/peering.schema.json': json.dumps(schema, indent=2)+'\n',
            'spec/fixtures/peering/cases.json': json.dumps(cases, indent=2)+'\n'}


if __name__ == '__main__':
    for name, text in outputs().items():
        path = ROOT / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
