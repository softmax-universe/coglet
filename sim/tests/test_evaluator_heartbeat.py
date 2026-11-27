"""Evaluator liveness conformance; synthetic controls are not reset causality."""
import json
from pathlib import Path
import sys
import tempfile

import pytest
from coglet_sim.evaluation import NativeTools, evaluate


def test_evaluator_health_during_silence(tmp_path, monkeypatch):
    # Real authenticated subprocess/socket, bounded receive watchdog, no sleeps.
    # A valid negotiation is followed by total runtime silence. The evaluator
    # must transmit scheduled health without a new inbound application frame.
    observed = tmp_path / 'observed.json'
    daemon = tmp_path / 'peer'
    daemon.write_text(f'#!{sys.executable}\n' + '''
import json, os, socket, sys
from coglet_surface.codec import decode, encode
from coglet_surface.transport import frame
WIRE = 'surface::surface-message'
def exact(c, n):
    data = bytearray()
    while len(data) < n:
        part = c.recv(n-len(data))
        if not part:
            raise EOFError()
        data.extend(part)
    return bytes(data)
def receive(c):
    return decode(WIRE, exact(c, int.from_bytes(exact(c, 4), 'big')))
path = sys.argv[2]
os.makedirs(os.path.dirname(path), exist_ok=True)
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.bind(path); s.listen(1)
c, _ = s.accept()
c.settimeout(2)
hello = receive(c)
c.sendall(frame(encode(WIRE, dict(type='hello_reply', body=dict(
    selection=dict(version='0.1'), correlation_id=hello['body']['correlation_id'])))))
seen = []
try:
    while 'health' not in seen:
        seen.append(receive(c)['type'])
except (TimeoutError, EOFError):
    pass
''' + f"\nopen({str(observed)!r}, 'w').write(json.dumps(seen))\n" + '''
c.close(); s.close()
''')
    daemon.chmod(0o700)
    with tempfile.TemporaryDirectory(prefix='hb-') as short:
        monkeypatch.setenv('TMPDIR', short)
        with pytest.raises((AssertionError, ConnectionError, TimeoutError)):
            evaluate(tmp_path, NativeTools(daemon, Path(sys.executable).resolve()),
                     b'wasm', world='cartpole', seed=42)
    assert 'health' in json.loads(observed.read_text())
    evidence = json.loads((tmp_path / 'native-failure.json').read_text())
    assert evidence['phase'] == 'loop'
    assert evidence['cleanup']['returncode'] is not None
