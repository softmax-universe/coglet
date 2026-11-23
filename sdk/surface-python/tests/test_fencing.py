import pytest
from coglet_surface.codec import decode, encode
from coglet_surface.surface import Operation, ProtocolError
from test_surface import COGLET, CID, make_surface, act

WIRE = 'surface::surface-message'


def messages(surface):
    return [decode(WIRE, raw) for raw in surface.drain()]


def test_kill_is_persistent_local_latch_and_retries_are_historical(tmp_path):
    surface, calls = make_surface(tmp_path)
    first = act(surface)
    surface.kill('operator off-switch')
    stopped = messages(surface)
    assert [m['type'] for m in stopped] == ['kill', 'health']
    assert stopped[1]['body']['state'] == 'stopped'
    assert act(surface, id='2' * 32)[0]['body']['error']['code'] == 'permission_denied'
    assert act(surface)[1]['body']['payload'] == first[1]['body']['payload']
    surface.close()
    surface, _ = make_surface(tmp_path, create=False)
    assert act(surface, id='3' * 32)[0]['body']['error']['code'] == 'permission_denied'
    surface.close()


@pytest.mark.parametrize('trigger,status', [('deadline', 'expired'), ('grant', 'cancelled'),
                                           ('disconnect', 'cancelled')])
def test_pending_work_stops_and_late_completion_cannot_revive(tmp_path, trigger, status):
    now = [0]
    surface, calls = make_surface(tmp_path, clock=lambda: now[0])
    op = surface.operations['expression.set_expression']
    surface.operations[op.name] = Operation(op.name, op.entry, op.direction, op.input,
        op.output, op.resources, op.prepare, lambda args, context: None)
    assert len(act(surface)) == 1
    stops = calls.count('stop')
    if trigger == 'deadline':
        now[0] = 1000000
        surface.tick()
    elif trigger == 'grant':
        surface.grant([])
    else:
        surface.disconnect()
    assert calls.count('stop') > stops
    assert not surface.complete(COGLET, CID, result={})
    row = surface.db.execute('SELECT outcome FROM actions').fetchone()
    from coglet_surface.codec import decode_profile
    assert decode_profile(row[0])['status'] == status
    surface.close()


def test_read_checks_input_and_output_bindings(tmp_path):
    surface, _ = make_surface(tmp_path)
    # Registration is supervisor-only and precedes connection in real use.
    surface.state = 'closed'
    surface.register(Operation('expression.state', 'expression.control', 'read',
        'surface::empty-map', 'expression::expression-state', (), lambda args, context: None,
        lambda args, context: {'expression': 'neutral', 'intensity': 0, 'remaining_ms': 0}))
    surface.state = 'ready'
    surface.grant([dict(coglet=COGLET, operations=['expression.state'], resources=[], expires=2000000)])
    surface.drain()
    request = dict(coglet=COGLET, capability='expression.state', args={}, grant_epoch=2,
                   deadline=1000000, correlation_id=CID)
    surface.receive(encode(WIRE, dict(type='read', body=request)))
    reply = messages(surface)[0]['body']
    assert reply['ok'] is True and reply['snapshot'] == 1
    request['args'] = {'forged': 1}
    request['correlation_id'] = '2' * 32
    surface.receive(encode(WIRE, dict(type='read', body=request)))
    assert messages(surface)[0]['body']['error']['code'] == 'invalid_argument'
    surface.close()


def test_crash_recovers_pending_without_execution(tmp_path):
    surface, _ = make_surface(tmp_path)
    op = surface.operations['expression.set_expression']
    surface.operations[op.name] = Operation(op.name, op.entry, op.direction, op.input,
        op.output, op.resources, op.prepare, lambda args, context: None)
    act(surface)
    # Crash: no graceful disconnect/close cancellation.
    surface.ledger.close()
    surface, calls = make_surface(tmp_path, create=False)
    result = act(surface)
    assert result[1]['body']['payload']['status'] == 'failed'
    assert result[1]['body']['payload']['error']['code'] == 'unavailable'
    assert not any(isinstance(c, dict) for c in calls)
    surface.close()


def test_speech_cannot_be_registered_without_its_native_leases(tmp_path):
    from coglet_surface.surface import Surface, Operation
    from test_surface import SURFACE
    surface = Surface(SURFACE, {COGLET}, tmp_path / 's', create=True,
                      clock=lambda: 0, stop=lambda: None)
    with pytest.raises(ValueError, match='resource'):
        surface.register(Operation('speech.speak_chunk', 'speech.control', 'action',
            'speech::speak-chunk', 'surface::empty-map', (),
            lambda args, context: None, lambda args, context: {}))
    surface.close()


def test_binding_registration_cannot_override_frozen_operation(tmp_path):
    surface, _ = make_surface(tmp_path)
    surface.state = 'closed'
    with pytest.raises(ValueError, match='binding'):
        surface.register(Operation('speech.config', 'speech.control', 'action',
            'surface::empty-map', 'surface::empty-map', (), lambda args, context: None, lambda args, context: {}))
    surface.close()
