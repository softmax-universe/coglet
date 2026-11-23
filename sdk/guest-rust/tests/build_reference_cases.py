"""Independent reference ABI vectors, never read by implementation adapter."""
import hashlib
import json
import pathlib
import sys
ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from conformance.wire import encode


def event(kind='test.echo'):
    return dict(source='surface:' + '1'*64, kind=kind, seq=0, logical_time=42,
                payload={'text': 'hello', 'u64': 2**64-1}, correlation_id='2'*32)


def request(guest, events=None, **changes):
    r = dict(guest=guest, events_hex=encode([event()] if events is None else events).hex(),
             step=0, logical_time=42, seed=7, files={})
    r.update(changes)
    return r


def echo(r, index=0):
    eid = hashlib.sha256(b'coglet-guest-id-v0' + b''.join(
        n.to_bytes(8, 'big') for n in (r['seed'], r['step'], index))).digest()[:16].hex()
    action = dict(id=eid, coglet='coglet:'+'a'*64, capability='test.echo',
                  args=event()['payload'], deadline=r['logical_time']+1000000,
                  correlation_id=event()['correlation_id'])
    return dict(op='emit', argument_hex=encode(action).hex())


def log(n):
    return dict(op='log', argument_hex=encode(dict(level='info', message=f'count={n}', fields={})).hex())


def case(name, r, ok, calls=None, files=None):
    return dict(name='reference-'+name, peer_role='guest-reference', steps=[
        dict(send=r), dict(expect=dict(ok=ok, calls=calls or [], files=r['files'] if files is None else files), within_ms=5000)])


def build():
    r = request('echo')
    cases = [case('echo', r, True, [echo(r)]),
             case('echo-two', request('echo', [event(), event()]), True, [echo(r), echo(r, 1)]),
             case('echo-filter', request('echo', [event('test.other')]), True)]
    r = request('echo', step=2**64-1, seed=2**64-1)
    cases.append(case('echo-u64', r, True, [echo(r)]))
    cases.append(case('echo-deadline-overflow', request('echo', logical_time=2**64-1), False))
    for guest in ('echo', 'counter'):
        cases.append(case(guest+'-empty', request(guest, []), False))
        bad = event(); bad['unknown'] = True
        cases.append(case(guest+'-malformed', request(guest, [bad]), False))
        cases.append(case(guest+'-reject', request(guest, host_result=2), False))
    cases.append(case('counter-new', request('counter'), True, [log(1)], {'/data/count': b'1'.hex()}))
    cases.append(case('counter-next', request('counter', step=1, files={'/data/count': b'1'.hex()}), True, [log(2)], {'/data/count': b'2'.hex()}))
    for text in ('invalid', str(2**64-1)):
        cases.append(case('counter-'+('corrupt' if text=='invalid' else 'overflow'), request('counter', files={'/data/count': text.encode().hex()}), False))
    cases.append(case('counter-write-rollback', request('counter', host_result=2, files={'/data/count': b'5'.hex()}), False))
    return json.dumps(cases, indent=2) + '\n'


if __name__ == '__main__':
    path = ROOT/'conformance/abi/reference-guests.json'
    content = build()
    if '--check' in sys.argv:
        assert path.read_text() == content, 'reference vectors stale'
    else:
        path.write_text(content)
