"""ADR-0030 local authority, selected wire dispatch, and durable identity."""
from dataclasses import FrozenInstanceError
import hashlib
import json
import os
import socket
import sqlite3
import threading

import pytest

from coglet_surface import Operation, ProtocolError, Rejection, Surface, TrustedExtensionCatalog
from coglet_surface.catalog import name
from coglet_surface.codec import decode_profile, encode_profile
from coglet_surface.transport import UnixListener, frame, serve
from test_surface import SURFACE, COGLET, CID

P02 = 'surface-0.2'
BASE = 'https://example.invalid/softmax/v1#'


def artifact():
    return dict(version=1, namespace='softmax', entries={'softmax.app': [], 'softmax.app_sensitive': ['softmax.control']},
        schemas={BASE + r: r for r in ('empty', 'query', 'state', 'observation')},
        rules={
            'empty': dict(op='record', fields=[]),
            'query': dict(op='record', fields=[dict(name='id', optional=False, type=dict(op='range', min=0, max=10))]),
            'state': dict(op='record', fields=[dict(name='title', optional=False, type=dict(op='size', min=0, max=64))]),
            'observation': dict(op='record', fields=[
                dict(name='operation', optional=False, type=dict(op='literal', value='softmax.app.state')),
                dict(name='args', optional=False, type=dict(op='ref', name='query')),
                dict(name='value', optional=False, type=dict(op='union', items=[dict(op='ref', name='state'), dict(op='ref', name='empty')]))])},
        operations={
            'softmax.app.state': dict(entry='softmax.app', direction='read', input=BASE+'query', output=BASE+'state', resources=[], single_owner=True),
            'softmax.app.rename': dict(entry='softmax.app_sensitive', direction='action', input=BASE+'state', output=BASE+'empty', resources=['softmax.control'], single_owner=True),
            'softmax.app.observation': dict(entry='softmax.app', direction='event', input=BASE+'observation', resources=[], single_owner=True,
                selection=dict(operation_field='operation', args_field='args', value_field='value', reads=['softmax.app.state']))})


def catalog(value=None, *, selected=P02):
    raw = json.dumps(value if value is not None else artifact(), ensure_ascii=True).encode()
    return TrustedExtensionCatalog(raw, hashlib.sha256(raw).hexdigest(), contract_profile=selected)


def surface(path, *, selected=P02, trusted=None, create=True, calls=None):
    calls = [] if calls is None else calls
    s = Surface(SURFACE, {COGLET}, path, create=create, stop=lambda: None,
                clock=lambda: 0, contract_profile=selected, trusted_catalog=trusted)
    if trusted:
        for n, b in trusted.artifact['operations'].items():
            def execute(args, ctx, n=n):
                calls.append((n, args))
                return {'title': 'hello'} if n.endswith('.state') else {}
            s.register(Operation(n, b['entry'], b['direction'], b['input'], b.get('output'),
                                 tuple(b['resources']), lambda a, c: None, execute))
    return s


def negotiate(s, version='0.2'):
    hello = decode_profile(s.open(CID))
    s.receive(encode_profile(dict(type='hello_reply', body=dict(selection=dict(version=version), correlation_id=CID))))
    return hello, [decode_profile(r) for r in s.drain()]


def grant(s, operations=None):
    s.grant([dict(coglet=COGLET, operations=operations or list(s.operations), resources=['softmax.control'], expires=2_000_000)])
    s.drain()


def request(s, kind, capability, args, cid=CID):
    body = dict(coglet=COGLET, capability=capability, args=args, deadline=1_000_000, correlation_id=cid)
    if kind == 'act':
        body['id'] = cid
        body = dict(action=body, grant_epoch=s._get('epoch'))
    else:
        body['grant_epoch'] = s._get('epoch')
    raw = encode_profile(dict(type=kind, body=body))
    s.receive(raw)
    return raw, [decode_profile(r) for r in s.drain()]


def test_local_contract_profile_defaults_and_unknowns(tmp_path):
    for i, bad in enumerate(('', None, '0.2', 'surface-0.3', 'SURFACE-0.2', [], 2)):
        with pytest.raises(ValueError, match='profile'):
            Surface(SURFACE, {COGLET}, tmp_path / str(i), create=True, stop=lambda: None, contract_profile=bad)
        assert not (tmp_path / str(i)).exists()
    s = Surface(SURFACE, {COGLET}, tmp_path/'default', create=True, stop=lambda: None, clock=lambda: 0)
    assert decode_profile(s.open(CID))['body']['offer']['versions'] == ['0.1']
    assert dict(s.db.execute('SELECT key,value FROM meta'))['contract_profile'] == 'surface-0.1'
    s.close()
    s = surface(tmp_path/'nocatalog')
    assert negotiate(s)[0]['body']['offer']['versions'] == ['0.2', '0.1']
    assert 'catalog_pin' not in dict(s.db.execute('SELECT key,value FROM meta'))
    s.close()
    with pytest.raises(ValueError, match='mismatch'):
        surface(tmp_path/'conflict', selected='surface-0.1', trusted=catalog())


@pytest.mark.parametrize('bad', ['softmax.app.state.more', 'softmax.application.state', 'other.app.state',
                                  'softmax.app.', 'softmax.app.State', 'softmax.app.é', 'softmax.app.state\n'] )
def test_exact_narrow_names(bad):
    with pytest.raises(ValueError):
        name(bad, P02)
    with pytest.raises(ValueError):
        name('softmax.app.state', 'surface-0.1')
    name('speech.start', P02)
    name('softmax.app.state', P02)


def test_local_contract_profile_catalog_v1_closed(tmp_path):
    c = catalog()
    with pytest.raises(FrozenInstanceError):
        c.contract_profile = 'surface-0.1'
    detached = c.artifact
    detached['rules']['query'] = dict(op='map', item=dict(op='primitive', name='tstr'))
    assert c.artifact == artifact()
    with pytest.raises(ValueError):
        TrustedExtensionCatalog(c.raw, '0'*64, contract_profile=P02)
    with pytest.raises(ValueError):
        TrustedExtensionCatalog(c.raw, c.sha256)
    mutations = [lambda a: a.update(contract_profile=P02), lambda a: a.update(version=2),
                 lambda a: a['rules']['query'].update(unreviewed=True),
                 lambda a: a['rules'].update(query=dict(op='map', item=dict(op='primitive', name='tstr'))),
                 lambda a: a['rules'].update(query=dict(op='ref', name='query')),
                 lambda a: a['rules'].update(query=dict(op='ref', name='missing')),
                 lambda a: a['operations']['softmax.app.state'].update(extra=True),
                 lambda a: a['entries'].update({'softmax.app.resource': []}),
                 lambda a: a['operations']['softmax.app.rename'].update(resources=['softmax.app.control'])]
    for mutate in mutations:
        a = artifact()
        mutate(a)
        with pytest.raises(ValueError):
            catalog(a)
    raw = b'{"version":1,"version":1}'
    with pytest.raises(ValueError, match='duplicate'):
        TrustedExtensionCatalog(raw, hashlib.sha256(raw).hexdigest(), contract_profile=P02)
    for selected in ('surface-0.1', P02):
        with pytest.raises(ValueError):
            Surface(SURFACE, {COGLET}, tmp_path/selected, create=True, stop=lambda: None,
                contract_profile=selected, extensions={'softmax.app.state': {}}, schemas={'app::any': {}})


def test_local_contract_profile_replay_pair_fenced(tmp_path):
    # Two-component catalog bytes are valid under both profiles, but identity is a pair.
    a = artifact()
    a['operations'] = {'softmax.state': a['operations']['softmax.app.state']}
    a['rules'].pop('observation')
    a['schemas'].pop(BASE+'observation')
    c01, c02 = catalog(a, selected='surface-0.1'), catalog(a)
    path = tmp_path/'pair'
    s = surface(path, selected='surface-0.1', trusted=c01)
    saved = dict(s.db.execute('SELECT key,value FROM meta'))
    assert saved['catalog_pin'] == c01.sha256 and bytes.fromhex(saved['catalog']) == c01.raw
    s.close()
    for kwargs in (dict(trusted=c02), dict(selected='surface-0.1'), dict(trusted=catalog())):
        with pytest.raises(ProtocolError, match='replay identity'):
            surface(path, create=False, **kwargs)
    s = surface(path, selected='surface-0.1', trusted=c01, create=False)
    s.close()
    # Legacy profile-less records mean exactly 0.1, never implicit opt-in.
    with sqlite3.connect(path) as db:
        db.execute("DELETE FROM meta WHERE key='contract_profile'")
    with pytest.raises(ProtocolError):
        surface(path, trusted=c02, create=False)
    s = surface(path, selected='surface-0.1', trusted=c01, create=False)
    s.close()
    with sqlite3.connect(path) as db:
        db.execute("INSERT INTO meta VALUES ('contract_profile','unknown')")
    with pytest.raises(ProtocolError):
        surface(path, selected='surface-0.1', trusted=c01, create=False)

    path = tmp_path/'legacy-no-catalog'
    s = surface(path, selected='surface-0.1')
    s.close()
    with sqlite3.connect(path) as db:
        db.execute("DELETE FROM meta WHERE key='contract_profile'")
    with pytest.raises(ProtocolError):
        surface(path, create=False)
    s = Surface(SURFACE, {COGLET}, path, stop=lambda: None)
    s.close()


def test_app_wire_optin_schema_dispatch_reopen_and_selected_event(tmp_path):
    calls = []
    c = catalog()
    path = tmp_path/'wire'
    s = surface(path, trusted=c, calls=calls)
    hello, caps = negotiate(s)
    assert caps[0]['body']['entries'][0]['schemas']['softmax.app.state'] == BASE+'state'
    grant(s)
    _, reply = request(s, 'read', 'softmax.app.state', {'id': 1})
    assert reply[0]['body']['value'] == {'title': 'hello'}
    _, reply = request(s, 'read', 'softmax.app.state', {}, cid='2'*32)
    assert reply[0]['body']['error']['code'] == 'invalid_argument'
    assert len(calls) == 1
    _, reply = request(s, 'act', 'softmax.app.rename', {'id': 1}, cid='4'*32)
    assert not reply[0]['body']['accepted']
    assert len(calls) == 1
    raw, reply = request(s, 'act', 'softmax.app.rename', {'title': 'new'})
    assert reply[0]['body']['accepted'] and reply[1]['body']['payload']['status'] == 'succeeded'
    before = len(calls)
    payload = dict(operation='softmax.app.state', args={'id': 1}, value={})
    with pytest.raises(ValueError):
        s.event(COGLET, 'softmax.app.observation', payload, epoch=s._get('epoch'))
    payload['value'] = {'title': 'ok'}
    s.event(COGLET, 'softmax.app.observation', payload, epoch=s._get('epoch'))
    assert decode_profile(s.drain()[0])['body']['kind'] == 'softmax.app.observation'
    grant(s, ['softmax.app.observation'])
    with pytest.raises(Rejection):
        s.event(COGLET, 'softmax.app.observation', payload, epoch=s._get('epoch'))
    s.close()
    s = surface(path, trusted=c, create=False, calls=calls)
    negotiate(s)
    # Receipts survive a restart but native work is not repeated; no fresh grants yet.
    s.receive(raw)
    assert decode_profile(s.drain()[0])['body']['accepted']
    assert len(calls) == before
    s.close()


def install_builtins(s, calls):
    s.register(Operation('expression.set_expression', 'expression.control', 'action',
        'expression::set-expression', 'surface::empty-map', ('display.face',), lambda a,c: None,
        lambda a,c: calls.append('face') or {}))
    s.register(Operation('speech.backchannel', 'speech.control', 'action',
        'speech::backchannel', 'surface::empty-map', ('audio.speaker',), lambda a,c: None,
        lambda a,c: calls.append('speech') or {}))


def test_local_contract_profile_downgrade_face_speech(tmp_path):
    calls = []
    c = catalog()
    s = surface(tmp_path/'down', trusted=c, calls=calls)
    install_builtins(s, calls)
    negotiate(s)
    grant(s, ['softmax.app.state', 'softmax.app.rename', 'softmax.app.observation'])
    s.event(COGLET, 'softmax.app.observation', dict(operation='softmax.app.state', args={'id': 1}, value={'title': 'ok'}), epoch=s._get('epoch'))
    assert s.queue
    s.disconnect()
    assert not s.queue and not s.grants
    _, caps = negotiate(s, '0.1')
    assert {e['name'] for e in caps[0]['body']['entries']} == {'expression.control', 'speech.control'}
    saved = dict(s.db.execute('SELECT key,value FROM meta'))
    assert saved['contract_profile'] == P02 and saved['catalog_pin'] == c.sha256
    with pytest.raises(ValueError):
        grant(s, ['softmax.app.rename'])
    s.grant([dict(coglet=COGLET, operations=['expression.set_expression', 'speech.backchannel'],
                  resources=['display.face', 'audio.speaker'], expires=2_000_000)])
    s.drain()
    _, reply = request(s, 'act', 'expression.set_expression', dict(expression='listening', intensity=500, duration_ms=1000))
    assert reply[0]['body']['accepted'] and 'face' in calls
    _, reply = request(s, 'act', 'speech.backchannel', dict(stream=CID, cue='ack'), cid='2'*32)
    assert reply[0]['body']['accepted'] and 'speech' in calls
    # Selected 0.1 wire validation rejects app names before dispatch, even with a stale grant map.
    s.grants[0]['operations'].append('softmax.app.rename')
    with pytest.raises(ProtocolError):
        request(s, 'act', 'softmax.app.rename', {'title': 'forbidden'}, cid='3'*32)
    assert not any(isinstance(v, tuple) for v in calls)
    s.close()


def test_real_unix_wire_optin_and_downgrade(tmp_path):
    # Authenticated listener and real framed serve path, not only Surface.receive.
    directory = tmp_path/'sock'
    directory.mkdir(mode=0o700)
    path = str(directory/'s')
    if len(path.encode()) >= 104:
        pytest.skip('platform socket path length')
    errors = []
    calls = []
    with UnixListener(path, runtime_uid=os.getuid(), runtime_pid=os.getpid()) as listener:
        def run():
            try:
                for version in ('0.2', '0.1'):
                    s = surface(tmp_path/'live', trusted=catalog(), create=version == '0.2', calls=calls)
                    install_builtins(s, calls)
                    with listener.accept() as peer:
                        def configured():
                            operations = ['softmax.app.state'] if version == '0.2' else ['expression.set_expression', 'speech.backchannel']
                            s.grant([dict(coglet=COGLET, operations=operations,
                                resources=[] if version == '0.2' else ['display.face', 'audio.speaker'], expires=2_000_000)])
                        try:
                            serve(s, peer.fileno(), peer.fileno(), hello_id=CID, configured=configured)
                        finally:
                            s.close()
            except BaseException as exc:
                errors.append(exc)
        thread = threading.Thread(target=run)
        thread.start()
        try:
            for version in ('0.2', '0.1'):
                with socket.socket(socket.AF_UNIX) as client:
                    client.settimeout(5)
                    client.connect(path)
                    def exact(size):
                        data = b''
                        while len(data) < size:
                            chunk = client.recv(size-len(data))
                            if not chunk:
                                raise EOFError
                            data += chunk
                        return data
                    def recv():
                        return decode_profile(exact(int.from_bytes(exact(4), 'big')))
                    assert recv()['body']['offer']['versions'] == ['0.2', '0.1']
                    client.sendall(frame(encode_profile(dict(type='hello_reply', body=dict(selection=dict(version=version), correlation_id=CID)))))
                    caps, grants = recv(), recv()
                    epoch = grants['body']['epoch']
                    if version == '0.2':
                        assert 'softmax.app' in {e['name'] for e in caps['body']['entries']}
                        body = dict(coglet=COGLET, capability='softmax.app.state', args={'id': 1}, deadline=1_000_000, correlation_id=CID, grant_epoch=epoch)
                        packet = frame(encode_profile(dict(type='read', body=body)))
                        client.sendall(packet[:3])
                        client.sendall(packet[3:])
                        assert recv()['body']['value'] == {'title': 'hello'}
                    else:
                        assert {e['name'] for e in caps['body']['entries']} == {'expression.control', 'speech.control'}
                        action = dict(id=CID, coglet=COGLET, capability='expression.set_expression',
                            args=dict(expression='listening', intensity=500, duration_ms=1000), deadline=1_000_000, correlation_id=CID)
                        client.sendall(frame(encode_profile(dict(type='act', body=dict(action=action, grant_epoch=epoch)))))
                        assert recv()['body']['accepted']
                        assert recv()['body']['payload']['status'] == 'succeeded'
                        action.update(id='2'*32, correlation_id='2'*32, capability='speech.backchannel', args=dict(stream=CID, cue='ack'))
                        client.sendall(frame(encode_profile(dict(type='act', body=dict(action=action, grant_epoch=epoch)))))
                        assert recv()['body']['accepted']
                        assert recv()['body']['payload']['status'] == 'succeeded'
        finally:
            thread.join(5)
        assert not thread.is_alive()
        assert not errors, errors
    assert 'face' in calls
    assert 'speech' in calls
