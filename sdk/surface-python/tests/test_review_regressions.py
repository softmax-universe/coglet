import pytest
from coglet_surface.codec import decode, encode
from coglet_surface.surface import Surface, ProtocolError, Operation
from coglet_surface.transport import Output, Frames
from test_surface import make_surface, SURFACE, COGLET, CID, act


@pytest.mark.parametrize('crossing', ['deadline', 'lease'])
def test_preparation_cannot_cross_admission_expiry_and_execute(tmp_path, crossing):
    now = [0]
    surface, calls = make_surface(tmp_path, clock=lambda: now[0])
    op = surface.operations['expression.set_expression']
    surface.operations[op.name] = Operation(op.name, op.entry, op.direction, op.input,
        op.output, op.resources, lambda args, ctx: now.__setitem__(0, 1000000 if crossing == 'deadline' else 2000000),
        lambda args, ctx: calls.append('effect') or {})
    result = act(surface)
    assert not result[0]['body']['accepted']
    assert 'effect' not in calls
    surface.close()


def test_postcommit_expiry_cannot_execute_effect(tmp_path):
    now = [0]
    surface, calls = make_surface(tmp_path, clock=lambda: now[0])
    op = surface.operations['expression.set_expression']
    surface.operations[op.name] = Operation(op.name, op.entry, op.direction, op.input,
        op.output, op.resources, op.prepare, lambda args, ctx: calls.append('effect') or {})
    emit = surface._emit
    def delay_receipt(kind, body):
        emit(kind, body)
        if kind == 'accept':
            now[0] = 1000000
    surface._emit = delay_receipt
    result = act(surface)
    assert result[0]['body']['accepted']
    assert result[1]['body']['payload']['status'] == 'expired'
    assert 'effect' not in calls
    surface.close()


def test_missing_ledger_stops_uncertain_native_effect_before_refusal(tmp_path):
    stops = []
    with pytest.raises(ProtocolError):
        Surface(SURFACE, {COGLET}, tmp_path / 'missing', stop=lambda: stops.append(True))
    assert stops == [True]


def test_kill_purges_queued_authority_and_prioritizes_stopped_health(tmp_path):
    surface, _ = make_surface(tmp_path)
    surface.grant([dict(coglet=COGLET, operations=['expression.set_expression'],
                        resources=['display.face'], expires=2000000)])
    old_grants = surface.drain()[0]
    surface._capabilities()
    surface.queue.append(old_grants)
    output = Output()
    output.append(old_grants, surface.revocation)
    surface.kill('off')
    queued = [decode('surface::surface-message', r) for r in surface.queue]
    assert [m['type'] for m in queued] == ['kill', 'health']
    output.revoke(surface.revocation, killed=True)
    for raw in surface.drain():
        output.append(raw, surface.revocation)
    result = []
    while output:
        chunk = bytes(output.peek())
        result += Frames().feed(chunk)
        output.consume(len(chunk))
    assert [decode('surface::surface-message', r)['type'] for r in result] == ['kill', 'health']
    surface.close()
