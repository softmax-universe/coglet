"""Closed ADR-0016 snapshots, with an independent CBOR encoder oracle."""
import hashlib

import cbor2
import pytest

from coglet_sim.worlds import Cartpole, Gridbot


@pytest.mark.parametrize('kind', [Gridbot, Cartpole])
def test_closed_world_hash(kind):
    world = kind(42)
    snapshot = world.snapshot()
    expected = hashlib.sha256(cbor2.dumps(snapshot, canonical=True)).hexdigest()
    assert world.state_hash() == expected
    assert world.state_hash() == kind(42).state_hash()
    assert world.state_hash() != kind(43).state_hash()
    world.step('east' if kind is Gridbot else 'right')
    assert world.state_hash() != expected
    assert world.state_hash() == hashlib.sha256(
        cbor2.dumps(world.snapshot(), canonical=True)).hexdigest()
