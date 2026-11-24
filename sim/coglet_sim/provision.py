"""Tell the duplex-audio supervisor which case is about to connect."""
import json
import os
import socket

with socket.socket(socket.AF_UNIX) as control:
    control.settimeout(10)
    control.connect(os.environ['COGLET_FIXTURE_CONTROL'])
    control.sendall(json.dumps({'case': os.environ['COGLET_CONFORMANCE_CASE']}).encode() + b'\n')
    assert control.recv(10) == b'ready\n'
