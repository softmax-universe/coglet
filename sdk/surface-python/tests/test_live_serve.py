"""Real-clock transport qualification, distinct from functional fixture time."""
import json
from pathlib import Path
import socket
import threading
import time

import pytest

from coglet_surface.codec import decode, encode
from coglet_surface.reference import ReferenceDevice
from coglet_surface.surface import ProtocolError, Surface
from coglet_surface.transport import frame, serve

ROOT = Path(__file__).resolve().parents[3]
WIRE = 'surface::surface-message'


@pytest.mark.parametrize('local_kill', [False, True])
def test_live_serve_polls_expiry_health_silence_and_kill(tmp_path, local_kill):
    config = json.loads((ROOT / 'spec/fixtures/surface/adapter-config.json').read_text())
    client, server = socket.socketpair()
    client.settimeout(5)  # Infrastructure bound; no claim of device response latency.
    errors = []
    stopped = threading.Event()

    def run():
        origin = time.monotonic_ns()
        clock = lambda: (time.monotonic_ns() - origin) // 1000
        device = ReferenceDevice(config, clock)
        def stop():
            device.stop()
            stopped.set()
        with server:
            surface = Surface(config['surface'], {g['coglet'] for g in config['grant']['entries']},
                tmp_path / 'ledger', create=True, clock=clock, stop=stop)
            try:
                device.install(surface, sorted({op for g in config['grant']['entries'] for op in g['operations']}))
                negotiated = [None]
                killed = [False]
                def configured():
                    entries = [dict(g, expires=clock() + 150_000) for g in config['grant']['entries']]
                    surface.grant(entries)
                    negotiated[0] = clock()
                def poll():
                    device.tick()
                    if local_kill and negotiated[0] is not None and not killed[0] and clock() - negotiated[0] >= 100_000:
                        killed[0] = True
                        surface.kill('local off-switch')
                serve(surface, server.fileno(), server.fileno(),
                      hello_id=config['hello']['body']['correlation_id'], configured=configured, poll=poll)
            except BaseException as exc:
                errors.append(exc)
            finally:
                surface.close()

    def exact(size):
        data = b''
        while len(data) < size:
            chunk = client.recv(size - len(data))
            if not chunk:
                raise EOFError
            data += chunk
        return data

    thread = threading.Thread(target=run)
    thread.start()
    try:
        hello = decode(WIRE, exact(int.from_bytes(exact(4), 'big')))
        assert hello['type'] == 'hello'
        client.sendall(frame(encode(WIRE, dict(type='hello_reply', body=dict(
            selection={'version': '0.1'}, correlation_id=hello['body']['correlation_id'])))))
        observed = []
        try:
            while True:
                observed.append(decode(WIRE, exact(int.from_bytes(exact(4), 'big'))))
        except EOFError:
            pass
        assert any(m['type'] == 'health' for m in observed), observed
        if local_kill:
            assert any(m['type'] == 'kill' for m in observed), observed
            assert any(m['type'] == 'health' and m['body']['state'] == 'stopped' for m in observed)
        else:
            assert any(m['type'] == 'grants' and not m['body']['entries'] for m in observed), observed
        assert stopped.is_set()
    finally:
        client.close()
        thread.join(timeout=5)
    assert not thread.is_alive()
    assert len(errors) == 1 and isinstance(errors[0], ProtocolError), errors
    assert str(errors[0]) == 'peer heartbeat timeout'
