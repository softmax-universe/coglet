import pytest
from coglet_surface.codec import decode, encode
from coglet_surface.surface import ProtocolError
from test_surface import make_surface, act, COGLET, CID


@pytest.mark.parametrize('changes,code', [({'deadline': 0}, 'deadline_exceeded'),
    ({'deadline': 60000001}, 'invalid_argument'),
    ({'args': {'expression': 'listening', 'intensity': True, 'duration_ms': 1}}, 'invalid_argument'),
    ({'coglet': 'coglet:' + 'c' * 64}, 'permission_denied')])
def test_admission_rejects_without_effect_or_outcome(tmp_path, changes, code):
    surface, calls = make_surface(tmp_path)
    result = act(surface, **changes)
    assert len(result) == 1
    assert result[0]['body']['error']['code'] == code
    assert not any(isinstance(c, dict) for c in calls)
    surface.close()


def test_queue_overflow_fails_closed_even_for_supervisor_events(tmp_path):
    surface, calls = make_surface(tmp_path)
    for _ in range(128):
        surface.health()
    with pytest.raises(ProtocolError, match='resource_exhausted'):
        surface.health()
    assert surface.state == 'closed' and not surface.grants
    surface.close()


def test_clock_rollback_and_heartbeat_timeout_stop_native_io(tmp_path):
    now = [1]
    surface, calls = make_surface(tmp_path, clock=lambda: now[0])
    now[0] = 0
    with pytest.raises(ProtocolError, match='rollback'):
        surface.tick()
    assert surface.state == 'closed' and not surface.grants
    now[0] = 1
    surface.close()


def test_idle_connection_requires_fresh_message_within_three_seconds(tmp_path):
    now = [0]
    surface, _ = make_surface(tmp_path, clock=lambda: now[0])
    now[0] = 3000000
    with pytest.raises(ProtocolError, match='heartbeat'):
        surface.tick()
    assert surface.state == 'closed' and not surface.grants
    surface.close()
