import os
import stat
import pytest
from coglet_surface.surface import Surface, ProtocolError
from test_surface import SURFACE, COGLET, make_surface, act


def test_ledger_is_private_and_single_writer(tmp_path):
    surface, _ = make_surface(tmp_path)
    assert stat.S_IMODE((tmp_path / 'state.db').stat().st_mode) == 0o600
    with pytest.raises(ProtocolError, match='in use'):
        Surface(SURFACE, {COGLET}, tmp_path / 'state.db', stop=lambda: None)
    surface.close()


def test_create_requires_fresh_file(tmp_path):
    surface, _ = make_surface(tmp_path)
    surface.close()
    with pytest.raises(ProtocolError, match='exists'):
        Surface(SURFACE, {COGLET}, tmp_path / 'state.db', create=True, stop=lambda: None)


def test_ledger_symlink_is_refused(tmp_path):
    surface, _ = make_surface(tmp_path)
    surface.close()
    path = tmp_path / 'link'
    path.symlink_to(tmp_path / 'state.db')
    with pytest.raises(ProtocolError, match='ledger'):
        Surface(SURFACE, {COGLET}, path, stop=lambda: None)


def test_atomic_invalid_grants_do_not_revoke_or_change_epoch(tmp_path):
    surface, _ = make_surface(tmp_path)
    for entries in ([dict(coglet=COGLET, operations=['unknown.name'], resources=[], expires=5)],
                    [dict(coglet=COGLET, operations=['expression.set_expression'],
                          resources=['display.face', 'display.face'], expires=5)]):
        with pytest.raises(ValueError):
            surface.grant(entries)
        assert surface._get('epoch') == 1
    assert act(surface)[0]['body']['accepted'] is True
    surface.close()
