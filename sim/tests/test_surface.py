import pytest
from coglet_surface.codec import decode, encode
from coglet_surface.surface import Rejection
from coglet_sim.worlds import Cartpole, Gridbot

SURFACE = 'surface:' + 'a'*64
PILOT = 'coglet:' + 'b'*64
CID = '1'*32
WIRE = 'surface::surface-message'


def make(tmp_path, kind=Gridbot):
    from coglet_sim.surface import WorldSurface
    world = kind(42)
    surface = WorldSurface(SURFACE, [PILOT], tmp_path/'ledger', world, clock=lambda:0)
    surface.open(CID)
    surface.receive(encode(WIRE, dict(type='hello_reply', body=dict(
        selection=dict(version='0.1'), correlation_id=CID))))
    surface.grant([dict(coglet=PILOT, operations=list(surface.operations), resources=[], expires=2000000)])
    surface.drain()
    return surface, world


def act(surface, *, direction='east', aid=CID, epoch=1, args=None):
    raw = encode(WIRE, dict(type='act', body=dict(grant_epoch=epoch, action=dict(
        id=aid, coglet=PILOT, capability=surface.action_name,
        args={'direction':direction} if args is None else args,
        deadline=1000000, correlation_id=aid))))
    surface.receive(raw)
    return surface.drain()


def test_world_surface_validation_and_retry(tmp_path):
    surface, world = make(tmp_path)
    surface.publish(epoch=1)
    initial = decode(WIRE, surface.drain()[0])
    assert initial['body']['kind'] == 'sim.gridbot_state'
    assert initial['body']['payload']['state_hash'] == world.state_hash()
    first = act(surface)
    assert decode(WIRE, first[0])['body']['accepted']
    assert world.steps == 1
    retry = act(surface)
    assert retry[0] == first[0]  # byte-identical durable acceptance
    assert len(retry) == 2  # receipt/outcome only, not another observation/transition
    assert world.steps == 1
    assert decode(WIRE, act(surface, aid='2'*32, args={'direction':'up'})[0])['body']['error']['code'] == 'invalid_argument'
    assert world.steps == 1
    surface.grant([])
    surface.drain()
    assert decode(WIRE, act(surface, aid='3'*32)[0])['body']['error']['code'] == 'permission_denied'
    with pytest.raises(Rejection):
        surface.publish(epoch=1)
    surface.close()


@pytest.mark.parametrize('kind,direction', [(Gridbot,'east'),(Cartpole,'right')])
def test_world_surface_termination_refuses_without_effect(tmp_path, kind, direction):
    surface, world = make(tmp_path, kind)
    world.done = True
    frames = act(surface, direction=direction)
    assert len(frames) == 1
    assert decode(WIRE, frames[0])['body']['error']['code'] == 'conflict'
    assert world.steps == 0
    surface.close()


@pytest.mark.parametrize('pilots', [[], [PILOT,PILOT], [PILOT,'coglet:'+'c'*64]])
def test_singleton_supervisor_binding(tmp_path, pilots):
    from coglet_sim.surface import WorldSurface, runtime_binding
    with pytest.raises(ValueError, match='exactly one'):
        WorldSurface(SURFACE, pilots, tmp_path/'ledger', Gridbot(42))
    with pytest.raises(ValueError, match='exactly one'):
        runtime_binding(SURFACE, pilots)
