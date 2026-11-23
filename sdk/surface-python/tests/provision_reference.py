"""Test supervisor hook: wait until the real reference device is provisioned.

Only setup uses this channel. Protocol traffic uses the private SDK Unix socket.
"""
import json
import os
import socket

with socket.socket(socket.AF_UNIX) as control:
    control.settimeout(10)
    control.connect(os.environ['COGLET_FIXTURE_CONTROL'])
    control.sendall(json.dumps({'case': os.environ['COGLET_CONFORMANCE_CASE'],
                              'kill_ms': os.environ.get('COGLET_FIXTURE_KILL_MS')}).encode() + b'\n')
    assert control.recv(10) == b'ready\n'
