"""Pinned independent Ruby CDDL oracle, never generated code or expected labels.

Schema(source).validate_cbor(raw) returns None or raises ValidationError for a
schema mismatch. ReferenceError means invalid schema, CBOR, or broken tooling;
it MUST NOT be treated as a negative fixture pass. Install spec/tests/Gemfile.lock
with Bundler before use. A bounded, serialized child amortizes Ruby startup.
"""
import atexit
import json
import os
from pathlib import Path
import selectors
import subprocess
import tempfile
import threading
import time


class ValidationError(ValueError):
    """Well-formed instance does not match the actual CDDL document."""


class ReferenceError(RuntimeError):
    """The independent reference could not produce a validation verdict."""


class _Worker:
    def __init__(self):
        self.lock = threading.Lock()
        self.process = None
        self.stderr = None
        self.owner_pid = os.getpid()

    def close(self):
        if self.process is not None and self.owner_pid == os.getpid():
            self.process.stdin.close()
            try:
                self.process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
            self.process.stdout.close()
            self.stderr.close()
        self.process = self.stderr = None

    def request(self, **request):
        with self.lock:
            if self.owner_pid != os.getpid():
                # A fork must not read/write its parent's protocol stream.
                self.process = self.stderr = None
                self.owner_pid = os.getpid()
            if self.process is None:
                here = Path(__file__).resolve().parent
                env = dict(os.environ, BUNDLE_GEMFILE=str(here / 'Gemfile'),
                           BUNDLE_FROZEN='true', CDDL_UNUSED_OK='1')
                self.stderr = tempfile.TemporaryFile()
                try:
                    self.process = subprocess.Popen(
                        ['ruby', str(here / 'reference_cddl.rb')], env=env,
                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr)
                except OSError as error:
                    self.stderr.close()
                    self.stderr = None
                    raise ReferenceError('Ruby CDDL startup failed') from error
            try:
                self.process.stdin.write((json.dumps(request) + '\n').encode())
                self.process.stdin.flush()
                deadline = time.monotonic() + 30
                data = bytearray()
                with selectors.DefaultSelector() as selector:
                    selector.register(self.process.stdout, selectors.EVENT_READ)
                    while not data.endswith(b'\n'):
                        if not selector.select(max(0, deadline - time.monotonic())):
                            raise ReferenceError('Ruby CDDL response timeout')
                        chunk = os.read(self.process.stdout.fileno(), 65536)
                        if not chunk:
                            self.stderr.seek(0)
                            raise ReferenceError('Ruby CDDL worker exited: ' + self.stderr.read(4096).decode(errors='replace'))
                        data.extend(chunk)
                        if len(data) > 65536:
                            raise ReferenceError('Ruby CDDL response too large')
                response = json.loads(data)
                if not isinstance(response, dict):
                    raise ReferenceError('Invalid Ruby CDDL response')
                if 'error' in response:
                    raise ReferenceError(response['error'])
                return response
            except (OSError, ValueError, ReferenceError) as error:
                self.close()
                if isinstance(error, ReferenceError):
                    raise
                raise ReferenceError('Ruby CDDL protocol failure') from error


_worker = _Worker()
atexit.register(_worker.close)


class Schema:
    def __init__(self, source):
        if not isinstance(source, str):
            raise TypeError('CDDL source must be text')
        self.source = source
        if _worker.request(op='compile', schema=source) != {'ok': True}:
            raise ReferenceError('Invalid Ruby CDDL compile response')

    def validate_cbor(self, raw):
        if not isinstance(raw, bytes):
            raise TypeError('CBOR input must be bytes')
        result = _worker.request(op='validate', schema=self.source, cbor_hex=raw.hex())
        if result == {'valid': False}:
            raise ValidationError('CBOR instance does not match CDDL')
        if result != {'valid': True}:
            raise ReferenceError('Invalid Ruby CDDL validation response')
