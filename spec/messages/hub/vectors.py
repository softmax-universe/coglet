"""Build test-only signing/framing vectors and shared harness case data."""
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

SPEC = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(SPEC / 'tests'))
from test_common import encode

OUT = SPEC / 'fixtures/hub'


def key(seed, path):
    private = bytes.fromhex('302e020100300506032b657004220420' + seed)
    path.write_bytes(private)
    public = subprocess.check_output(['openssl', 'pkey', '-inform', 'DER', '-in', str(path), '-pubout', '-outform', 'DER'])
    return public[-32:].hex()


with tempfile.TemporaryDirectory() as td:
    p = Path(td)
    cog = key('01' * 32, p / 'cog')
    hub = key('02' * 32, p / 'hub')
    device = key('03' * 32, p / 'device')
    cid = 'coglet:' + cog
    vectors = []
    for purpose in ['delegation', 'hub-ref', 'published-ref', 'logs-ref']:
        statement = {'purpose': purpose, 'hub': hub, 'coglet': cid, 'generation': 1}
        if purpose == 'delegation':
            statement.update(device=device, expires=4102444800, methods=['hub.events', 'hub.actions'])
        else:
            ref = {'hub-ref': 'refs/heads/hub', 'published-ref': 'refs/published/main', 'logs-ref': 'refs/heads/logs'}[purpose]
            statement.update(ref=ref, old=None, new='d' * 40, archive_sha256='e' * 64)
        preimage = ('coglet/hub/0:' + purpose + '\n').encode() + encode(statement)
        (p / 'msg').write_bytes(preimage)
        signer = hub if purpose == 'hub-ref' else cog
        priv = p / ('hub' if purpose == 'hub-ref' else 'cog')
        sig = subprocess.check_output(['openssl', 'pkeyutl', '-sign', '-keyform', 'DER', '-inkey', str(priv), '-rawin', '-in', str(p / 'msg')])
        vectors.append({'statement': statement, 'signer': signer, 'signature': sig.hex(), 'preimage_hex': preimage.hex()})
    # A real self-contained bundle and signed archive, not an invented git response.
    repo = p / 'repo'
    repo.mkdir()
    subprocess.run(['git', 'init', '-q', str(repo)], check=True)
    (repo / 'bin').mkdir()
    (repo / 'bin/step.wasm').write_bytes(bytes.fromhex('0061736d01000000'))
    (repo / 'manifest').write_text('{}\n')
    env = dict(os.environ, GIT_AUTHOR_NAME='Fixture', GIT_AUTHOR_EMAIL='fixture@example.invalid',
               GIT_COMMITTER_NAME='Fixture', GIT_COMMITTER_EMAIL='fixture@example.invalid',
               GIT_AUTHOR_DATE='2026-01-01T00:00:00Z', GIT_COMMITTER_DATE='2026-01-01T00:00:00Z')
    def git(*args):
        return subprocess.check_output(['git', '-C', str(repo), *args], env=env)
    git('add', '.')
    git('-c', 'commit.gpgsign=false', 'commit', '-qm', 'Fixture code artifact')
    oid = git('rev-parse', 'HEAD').decode().strip()
    git('update-ref', 'refs/heads/hub', oid)
    archive = git('archive', '--format=tar', oid)
    git('bundle', 'create', str(p / 'bundle'), 'refs/heads/hub')
    bundle = (p / 'bundle').read_bytes()
    git_statement = {'purpose': 'hub-ref', 'hub': hub, 'coglet': cid, 'generation': 1,
                     'ref': 'refs/heads/hub', 'old': None, 'new': oid,
                     'archive_sha256': hashlib.sha256(archive).hexdigest()}
    preimage = b'coglet/hub/0:hub-ref\n' + encode(git_statement)
    (p / 'msg').write_bytes(preimage)
    sig = subprocess.check_output(['openssl', 'pkeyutl', '-sign', '-keyform', 'DER',
             '-inkey', str(p / 'hub'), '-rawin', '-in', str(p / 'msg')])
    git_update = {'statement': git_statement, 'signer': hub, 'signature': sig.hex()}
    git_fixture = {'update': git_update, 'size': len(bundle), 'sha256': hashlib.sha256(bundle).hexdigest(),
                   'bundle_hex': bundle.hex(), 'archive_hex': archive.hex()}
(OUT / 'git.json').write_text(json.dumps(git_fixture, indent=2) + '\n')
(OUT / 'signatures.json').write_text(json.dumps(vectors, indent=2) + '\n')
framing = [
    ('short-prefix', '0000'), ('zero-length', '00000000'), ('oversize-prefix', '00100001'),
    ('truncated-body', '00000002a0'), ('trailing-frame', '00000001a000'),
    ('indefinite-map', '00000002bfff'), ('byte-string', '0000000140'),
    ('non-shortest-int', '000000021800'), ('duplicate-key', '00000007a2616101616102')]
(OUT / 'framing.json').write_text(json.dumps([{'name': n, 'hex': h} for n, h in framing], indent=2) + '\n')

cases = []
serial = 0

def request(method, params=None, target: str | None=cid):
    global serial
    serial += 1
    result = {'method': method, 'target': target, 'correlation_id': f'{serial:032x}',
              'timeout_ms': 2000, 'params': params or {}}
    if method in ['hub.install', 'hub.start', 'hub.stop', 'hub.git.push', 'hub.actions', 'debug.pause', 'debug.step', 'hub.receipt', 'hub.cancel']:
        result['context'] = {'actor': 'hub:' + hub}
    return result


def exchange(req, match):
    return [{'send': req}, {'match': dict({'correlation_id': req['correlation_id']}, **match), 'within_ms': 2000}]


def case(name, steps):
    first = steps[0]['send']['method']
    role = 'device' if first in ['hub.enroll', 'hub.events', 'hub.health'] else 'hub'
    # Driver metadata is outside send and is never emitted onto the wire.
    if any('type' in step.get('send', {}) for step in steps):
        for step in steps:
            frame = step.get('send')
            if frame and (frame.get('method') == 'hub.git.push' or 'type' in frame):
                step['stream'] = int(frame['correlation_id'], 16)
                step['finish'] = frame.get('type') == 'end'
    cases.append({'name': name, 'peer_role': role, 'fixtures': ['hub.' + name], 'steps': steps})


def err(req, code):
    return exchange(req, {'type': 'error', 'error.code': code})

# Each case gets isolated fixture provisioning described in conformance/hub/README.md.
enroll = request('hub.enroll', {'offer': {'versions': ['0.1']}, 'invite': '1' * 64}, None)
case('enroll-one-use', exchange(enroll, {'type': 'reply', 'result.selection.version': '0.1'}) +
     err(request('hub.enroll', enroll['params'], None), 'unauthenticated'))
case('enroll-reconnect', exchange(request('hub.enroll', {'offer': {'versions': ['0.1']}, 'invite': None}, None),
                                 {'type': 'reply', 'result.selection.version': '0.1'}))
case('enroll-expired', err(enroll, 'unauthenticated'))
case('enroll-window-closed', err(enroll, 'unauthenticated'))
case('enroll-wrong-node', err(enroll, 'unauthenticated'))
case('enroll-unsupported-version', err(request('hub.enroll', {'offer': {'versions': ['9.0']}, 'invite': '1' * 64}, None), 'unsupported_version'))
case('start-before-enroll', err(request('hub.start'), 'unauthenticated'))
case('lifecycle-start-stop', exchange(request('hub.start'), {'result.state': 'applied'}) +
     exchange(request('hub.status'), {'result.state': 'running'}) +
     exchange(request('hub.stop'), {'result.state': 'applied'}) +
     exchange(request('hub.status'), {'result.state': 'stopped'}))
write = request('debug.write', {'path': '/data/test.txt', 'text': 'intervention', 'expected_hash': 'e' * 64,
                              'actor': 'hub:' + hub})
case('read-grant-is-not-write', err(write, 'permission_denied') +
     exchange(request('debug.read', {'path': '/data/test.txt', 'offset': 0, 'limit': 64}), {'result.text': 'original'}))
case('write-stale-snapshot', err(write, 'conflict'))
for path, name in [('/data/../code/step.wasm', 'debug-traversal'), ('/data/inbox.jsonl', 'debug-inbox-protected')]:
    r = copy.deepcopy(write)
    r['params']['path'] = path
    case(name, err(r, 'permission_denied'))
case('pause-single-step', exchange(request('debug.pause'), {'result.state': 'applied'}) +
     exchange(request('debug.step'), {'result.step': 1, 'result.receipt.state': 'applied'}) +
     exchange(request('hub.status'), {'result.state': 'paused', 'result.step': 1}))
message = {'id': 'f' * 32, 'sender': 'hub:' + hub, 'event': {'source': 'hub:' + hub, 'kind': 'hub.note',
           'seq': 0, 'logical_time': 0, 'payload': {'text': 'hello\nworld'}, 'correlation_id': 'f' * 32}}
inbox = request('hub.inbox', {'message': message, 'actor': 'hub:' + hub})
case('inbox-durable-dedup', exchange(inbox, {'result.state': 'applied'}) + exchange(inbox, {'result.state': 'applied'}) +
     exchange(request('debug.read', {'path': '/data/inbox.jsonl', 'offset': 0, 'limit': 4096}), {'result.eof': True}))
conflict = copy.deepcopy(inbox)
conflict['params']['message']['event']['payload']['text'] = 'changed'
case('inbox-conflicting-id', exchange(inbox, {'result.state': 'applied'}) + err(conflict, 'conflict'))
case('queue-backpressure', err(inbox, 'resource_exhausted'))
case('unknown-receipt', err(request('hub.receipt', {'operation_id': '9' * 32}), 'not_found'))
case('cross-owner-receipt', err(request('hub.receipt', {'operation_id': '8' * 32}), 'permission_denied'))
event = {'source': cid, 'kind': 'test.event', 'seq': 0, 'logical_time': 0, 'payload': {}, 'correlation_id': '1' * 32}
events = request('hub.events', {'events': [event]})
case('event-batch-dedup', exchange(events, {'result.next_seq': 1}) + exchange(events, {'result.next_seq': 1}))
gap = copy.deepcopy(event)
gap['seq'] = 2
case('event-gap', err(request('hub.events', {'events': [gap]}), 'conflict'))
action = {'id': '2' * 32, 'coglet': cid, 'capability': 'test.move', 'args': {}, 'deadline': 0, 'correlation_id': '2' * 32}
case('action-expired', exchange(request('hub.actions', {'action': action}), {'result.accepted': False, 'result.error.code': 'deadline_exceeded'}))
case('health-minimal', exchange(request('hub.health', {'device': device, 'uptime_ms': 1, 'queue_depth': 0,
     'available_bytes': 1048576, 'coglets': []}, None), {'type': 'reply', 'result': {}}))
# Crypto failure is checked before transfer credit or any object import.
ref = {k: v for k, v in vectors[1].items() if k != 'preimage_hex'}
ref['signature'] = '0' * 128
case('git-invalid-signature', err(request('hub.git.push', {'update': ref, 'size': 1, 'sha256': 'e' * 64}), 'unauthenticated'))
push = request('hub.git.push', {k: git_fixture[k] for k in ['update', 'size', 'sha256']})
op = push['correlation_id']
chunk = {'type': 'chunk', 'correlation_id': op, 'offset': 0, 'hex': bundle.hex()}
end = {'type': 'end', 'correlation_id': op}
case('git-push-activate', [{'send': push}, {'match': {'type': 'credit', 'correlation_id': op,
     'bytes': len(bundle)}, 'within_ms': 2000}, {'send': chunk}, {'send': end},
     {'match': {'type': 'reply', 'correlation_id': op, 'result.state': 'applied'}, 'within_ms': 2000}] +
     exchange(request('hub.install', {'delegation': {k: v for k, v in vectors[0].items() if k != 'preimage_hex'},
                                     'initial': git_update}), {'result.state': 'applied'}) +
     exchange(request('hub.status'), {'result.live_commit': oid, 'result.state': 'stopped'}))
bad_hash = copy.deepcopy(push)
bad_hash['params']['sha256'] = '0' * 64
case('git-bundle-hash-mismatch', [{'send': bad_hash}, {'match': {'type': 'credit'}, 'within_ms': 2000},
    {'send': chunk}, {'send': end}, {'match': {'type': 'error', 'error.code': 'invalid_argument'}, 'within_ms': 2000}])
bad_offset = copy.deepcopy(chunk)
bad_offset['offset'] = 1
case('git-offset-gap', [{'send': push}, {'match': {'type': 'credit'}, 'within_ms': 2000},
    {'send': bad_offset}, {'match': {'type': 'error', 'error.code': 'invalid_argument'}, 'within_ms': 2000}])
queued = copy.deepcopy(inbox)
case('inbox-boundary-queued', exchange(queued, {'result.state': 'queued'}) +
     exchange(request('hub.cancel', {'operation_id': queued['correlation_id']}), {'result.state': 'cancelled'}))
case('single-step-requires-pause', err(request('debug.step'), 'conflict'))
case('start-missing-capability', err(request('hub.start'), 'permission_denied'))
(SPEC.parent / 'conformance/hub/cases.json').write_text(json.dumps(cases, indent=2) + '\n')
print(f'Generated {len(vectors)} signing vectors, {len(framing)} framing negatives, {len(cases)} harness cases')
