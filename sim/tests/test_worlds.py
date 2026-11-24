import pytest


def test_gridbot_seed_and_boundaries():
    from coglet_sim.worlds import Gridbot

    a, b = Gridbot(7, width=8, height=6), Gridbot(7, width=8, height=6)
    assert a.snapshot() == b.snapshot()
    assert a.snapshot() != Gridbot(8, width=8, height=6).snapshot()
    assert (a.x, a.y) != (a.goal_x, a.goal_y)
    for _ in range(64):
        for world in (a, b):
            if not world.done:
                world.step('west')
        assert a.snapshot() == b.snapshot()
    assert a.x == 0 or a.done
    assert 0 <= a.y < 6
    before = a.snapshot()
    with pytest.raises(ValueError):
        a.step('teleport')
    assert a.snapshot() == before
    while not b.done:
        b.step('east' if b.x < b.goal_x else 'west' if b.x > b.goal_x
               else 'south' if b.y < b.goal_y else 'north')
    assert (b.x, b.y) == (b.goal_x, b.goal_y)
    before = b.snapshot()
    with pytest.raises(ValueError, match='terminated'):
        b.step('east')
    assert b.snapshot() == before


@pytest.mark.parametrize('seed,width,height', [(-1, 8, 8), (True, 8, 8),
    (2**64, 8, 8), (0, 1, 8), (0, 8, 65), (0, 8.0, 8)])
def test_gridbot_rejects_invalid_configuration(seed, width, height):
    from coglet_sim.worlds import Gridbot
    with pytest.raises(ValueError):
        Gridbot(seed, width=width, height=height)


def test_cartpole_seed_and_episode():
    from coglet_sim.worlds import Cartpole
    a, b = Cartpole(7), Cartpole(7)
    assert a.snapshot() == b.snapshot()
    assert a.snapshot() != Cartpole(8).snapshot()
    while not a.done:
        assert a.step('right') == b.step('right')
        assert a.steps <= 10000
        for key in ('x', 'velocity', 'angle', 'angular_velocity'):
            assert type(a.snapshot()[key]) is int
            assert -(2**31) <= a.snapshot()[key] < 2**31
    assert a.steps > 0
    before = a.snapshot()
    with pytest.raises(ValueError, match='terminated'):
        a.step('right')
    assert a.snapshot() == before
    c = Cartpole(7, max_steps=1)
    c.step('left')
    assert c.done and c.steps == 1


@pytest.mark.parametrize('seed,max_steps', [(-1, 500), (True, 500),
    (2**64, 500), (0, 0), (0, 10001), (0, 1.0)])
def test_cartpole_rejects_invalid_configuration(seed, max_steps):
    from coglet_sim.worlds import Cartpole
    with pytest.raises(ValueError):
        Cartpole(seed, max_steps=max_steps)
