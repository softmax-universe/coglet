"""Build explicit semantic controls; snapshots are spec inputs, not real grants."""
import copy
import json
from pathlib import Path
import build_peering

ROOT = Path(__file__).resolve().parents[2]


def outputs():
    shape = json.loads(build_peering.outputs()['spec/fixtures/peering/cases.json'])
    request = copy.deepcopy(next(c['value'] for c in shape if c['name'] == 'private-route'))
    operation = request['capability']
    state = dict(mutual_enrollment=True, transport_peer=request['sender'], local_router=request['receiver'],
                 household=request['household'], attending=request['sender'],
                 origin_binding=[request['sender'], request['origin'], request['actor'], request.get('ticket')],
                 route=[request[k] for k in ('household', 'sender', 'receiver', 'origin', 'target', 'capability')],
                 killed=False, authority_current=True, clock_mapped=True, now=999,
                 sending_policy=[operation], receiving_policy=[operation], delegation=[operation], adapter_grants=[operation],
                 private_record=request['private'],
                 actor_owner=[request['actor'], request['private']['owner'], request['private']['source'], 1],
                 private_active=True)
    cases = []
    def add(name, request_changes=None, state_changes=None, result='permission_denied'):
        r, s = copy.deepcopy(request), copy.deepcopy(state)
        r.update(request_changes or {})
        s.update(state_changes or {})
        cases.append(dict(name=name, request=r, state=s, result=result))
    add('authorized-private', result='accepted')
    add('authorized-public', state_changes={'private_record': None}, result='accepted')
    del cases[-1]['request']['private']
    for layer in ('sending_policy', 'receiving_policy', 'delegation', 'adapter_grants'):
        add('authority-widening-'+layer, state_changes={layer: []})
    add('unauthenticated-peer', state_changes={'mutual_enrollment': False}, result='unauthenticated')
    add('transport-key-mismatch', state_changes={'transport_peer': 'hub:'+'0'*64}, result='unauthenticated')
    add('cross-owner-without-binding', state_changes={'actor_owner': None})
    add('foreign-target', request_changes={'target': 'coglet:'+'0'*64})
    add('stale-source-epoch', request_changes={'private': dict(request['private'], epoch=0)})
    zero = dict(request['private'], epoch=0)
    add('consistent-zero-epoch', request_changes={'private': zero},
        state_changes={'private_record': zero,
                       'actor_owner': [request['actor'], zero['owner'], zero['source'], 0]})
    add('fenced-source', state_changes={'private_active': False})
    add('omitted-private-cannot-declassify')
    del cases[-1]['request']['private']
    add('expired-deadline', state_changes={'now': 1000}, result='deadline_exceeded')
    add('clock-unmapped', state_changes={'clock_mapped': False}, result='deadline_exceeded')
    add('receiver-killed', state_changes={'killed': True})
    add('revoked-authority', state_changes={'authority_current': False})
    add('wrong-household', request_changes={'household': '0'*32})
    add('forged-origin-actor', request_changes={'actor': '0'*64})
    add('ended-ticket', state_changes={'origin_binding': []})
    return {'spec/fixtures/peering/authority.fixture': json.dumps(cases, indent=2)+'\n'}


if __name__ == '__main__':
    for path, text in outputs().items():
        (ROOT / path).write_text(text)
