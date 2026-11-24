"""Sandbox operation validation is pinned, never a generic profile-map fallback."""
import pytest
from coglet_surface.codec import validate
from coglet_surface.surface import CATALOG


def test_sandbox_release_catalog():
    assert CATALOG['sim.gridbot_move'] == dict(entry='sim.gridbot', direction='action',
        input='sandbox:gridbot-move', output='surface:empty-map')
    validate('sandbox::gridbot-move', {'direction': 'north'})
    for args in ({'direction':'up'}, {'direction':1}, {'direction':'north','extra':True}, {}):
        with pytest.raises(ValueError):
            validate('sandbox::gridbot-move', args)
