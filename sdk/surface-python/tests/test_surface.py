from pathlib import Path

import pytest

from coglet_surface.codec import decode, encode

SURFACE = 'surface:' + 'a' * 64
COGLET = 'coglet:' + 'b' * 64
CID = '1' * 32


def test_negotiation_requires_bound_peer_and_selected_version(tmp_path):
    from coglet_surface.surface import Surface, ProtocolError

    surface = Surface(SURFACE, {COGLET}, tmp_path / 'state.db', create=True,
                      clock=lambda: 0, stop=lambda: None)
    hello = decode('surface::surface-message', surface.open(CID))
    assert hello == {'type': 'hello', 'body': {'surface': SURFACE,
                     'offer': {'versions': ['0.1']}, 'clock': 0, 'correlation_id': CID}}
    with pytest.raises(ProtocolError):
        surface.receive(encode('surface::surface-message', {'type': 'hello_reply', 'body': {
            'selection': {'version': '0.2'}, 'correlation_id': CID}}))
    surface.close()


def make_surface(tmp_path, *, create=True, clock=lambda: 0):
    from coglet_surface.surface import Surface, Operation
    calls = []
    surface = Surface(SURFACE, {COGLET}, tmp_path / 'state.db', create=create,
                      clock=clock, stop=lambda: calls.append('stop'))
    surface.register(Operation('expression.set_expression', 'expression.control',
        'action', 'expression::set-expression', 'surface::empty-map',
        ('display.face',), lambda args, context: None, lambda args, context: calls.append(args) or {}))
    surface.open(CID)
    surface.receive(encode('surface::surface-message', {'type': 'hello_reply', 'body': {
        'selection': {'version': '0.1'}, 'correlation_id': CID}}))
    surface.grant([{'coglet': COGLET, 'operations': ['expression.set_expression'],
                    'resources': ['display.face'], 'expires': 2000000}])
    surface.drain()
    return surface, calls


def act(surface, **changes):
    action = dict(id=CID, coglet=COGLET, capability='expression.set_expression',
                  args=dict(expression='listening', intensity=500, duration_ms=1000),
                  deadline=1000000, correlation_id=CID)
    action.update(changes)
    surface.receive(encode('surface::surface-message', {'type': 'act', 'body': {
        'grant_epoch': 1, 'action': action}}))
    return [decode('surface::surface-message', raw) for raw in surface.drain()]


def test_durable_retry_never_reexecutes_and_conflicts(tmp_path):
    surface, calls = make_surface(tmp_path)
    first = act(surface)
    assert first[0]['body']['accepted'] is True
    assert first[1]['body']['payload']['status'] == 'succeeded'
    retry = act(surface)
    assert retry[0] == first[0]
    assert retry[1]['body']['payload'] == first[1]['body']['payload']
    assert retry[1]['body']['seq'] > first[1]['body']['seq']
    assert len([c for c in calls if isinstance(c, dict)]) == 1
    assert act(surface, deadline=999999)[0]['body']['error']['code'] == 'conflict'
    surface.close()
    surface, calls = make_surface(tmp_path, create=False)
    restarted = act(surface)
    assert restarted[0] == first[0]
    assert restarted[1]['body']['seq'] > retry[1]['body']['seq']
    assert not any(isinstance(c, dict) for c in calls)
    surface.close()
