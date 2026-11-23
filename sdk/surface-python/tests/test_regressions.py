import pytest
from coglet_surface.codec import decode, encode
from coglet_surface.surface import Surface, ProtocolError
from test_surface import SURFACE, COGLET, CID, make_surface, act
from test_fencing import messages


def test_existing_empty_ledger_cannot_be_silently_recreated(tmp_path):
    ledger = tmp_path / 'lost.db'
    ledger.touch()
    with pytest.raises(ProtocolError, match='ledger'):
        Surface(SURFACE, {COGLET}, ledger, stop=lambda: None)


def test_rejected_action_deadline_does_not_stop_unrelated_io(tmp_path):
    surface, calls = make_surface(tmp_path)
    assert act(surface, deadline=0)[0]['body']['accepted'] is False
    stops = calls.count('stop')
    surface.tick()
    surface.tick()
    assert calls.count('stop') == stops
    surface.close()


def test_grant_revocation_delivers_cancellation_not_silent_drop(tmp_path):
    from coglet_surface.surface import Operation
    surface, _ = make_surface(tmp_path)
    op = surface.operations['expression.set_expression']
    surface.operations[op.name] = Operation(op.name, op.entry, op.direction, op.input,
        op.output, op.resources, op.prepare, lambda args, context: None)
    act(surface)
    surface.grant([])
    result = messages(surface)
    assert any(m['type'] == 'event' and m['body']['payload']['status'] == 'cancelled' for m in result)
    surface.close()


def test_reference_expression_expires_without_read():
    from coglet_surface.reference import ReferenceDevice
    clock = [0]
    device = ReferenceDevice({}, lambda: clock[0])
    device.execute('expression.set_expression', dict(expression='listening', intensity=500, duration_ms=1))
    clock[0] = 2000
    device.tick()
    assert device.expression == 'neutral' and device.intensity == 0


def test_late_success_after_lease_expiry_is_cancelled(tmp_path):
    from coglet_surface.surface import Operation
    now = [0]
    surface, _ = make_surface(tmp_path, clock=lambda: now[0])
    op = surface.operations['expression.set_expression']
    surface.operations[op.name] = Operation(op.name, op.entry, op.direction, op.input,
        op.output, op.resources, op.prepare, lambda args, context: None)
    surface.grant([dict(coglet=COGLET, operations=[op.name], resources=['display.face'], expires=10)])
    surface.drain()
    # New action under the replacement epoch.
    request = dict(type='act', body=dict(grant_epoch=2, action=dict(id=CID, coglet=COGLET,
        capability=op.name, args=dict(expression='listening', intensity=500, duration_ms=1000),
        deadline=1000000, correlation_id=CID)))
    surface.receive(encode('surface::surface-message', request))
    surface.drain()
    now[0] = 10
    surface.complete(COGLET, CID, result={})
    assert messages(surface)[0]['body']['payload']['status'] == 'cancelled'
    surface.close()


def test_malformed_frame_immediately_disconnects_and_stops(tmp_path):
    surface, calls = make_surface(tmp_path)
    stops = calls.count('stop')
    with pytest.raises(ProtocolError):
        surface.receive(b'\x00')
    assert surface.state == 'closed' and not surface.grants
    assert calls.count('stop') > stops
    surface.close()
