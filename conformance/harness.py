"""Language-neutral case runner. No implementation code is imported."""
import argparse
import asyncio
import json
import os
from pathlib import Path
import re
import shlex
import signal
import sys
import tempfile
import time
import tomllib
import xml.etree.ElementTree as ET

from wire import decode, encode, read_frame, write_frame

ROOT = Path(__file__).resolve().parent.parent
SUITES = ROOT / 'conformance'
NAME = re.compile(r'[a-z][a-z0-9_-]*\Z')
LABEL = re.compile(r'[a-z][a-z0-9_.-]*\Z')
# ADR-0030 section 1a: trusted local provisioning identity, never a wire field.
PROFILES = ('surface-0.1', 'surface-0.2')
DEFAULT_PROFILE = 'surface-0.1'
PROFILE_ENV = 'COGLET_CONFORMANCE_CONTRACT_PROFILE'


def profile_value(value, what):
    """Exact profile spelling only: no case folding, trimming or fallback."""
    if type(value) is not str or value not in PROFILES:
        raise ValueError(f'{what} must be exactly one of {", ".join(PROFILES)}')
    return value


def load_json(path):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError(f'duplicate JSON key {key}')
            result[key] = value
        return result
    return json.loads(path.read_text(), object_pairs_hook=pairs,
                      parse_constant=lambda s: (_ for _ in ()).throw(ValueError(s)))


def millis(value):
    if type(value) not in (int, float) or not 0 <= value <= 300000:
        raise ValueError('time must be a finite number in 0..300000 ms')
    return value / 1000


def validate_case(case):
    if not isinstance(case, dict) or set(case) - {'name', 'steps', 'timeout_ms', 'description', 'peer_role', 'fixtures', 'contract_profile'}:
        raise ValueError('case must have name, steps and optional timeout_ms/description')
    if not isinstance(case.get('name'), str) or not case['name']:
        raise ValueError('case name must be nonempty text')
    if not isinstance(case.get('steps'), list) or not case['steps']:
        raise ValueError('case steps must be a nonempty array')
    millis(case.get('timeout_ms', 10000))
    profile_value(case.get('contract_profile', DEFAULT_PROFILE), 'case contract_profile')
    if not isinstance(case.get('peer_role', 'runtime'), str) or not NAME.fullmatch(case.get('peer_role', 'runtime')):
        raise ValueError('invalid peer_role')
    labels = case.get('fixtures', [])
    if not isinstance(labels, list) or any(not isinstance(s, str) or not LABEL.fullmatch(s) for s in labels) or len(set(labels)) != len(labels):
        raise ValueError('fixtures must be unique label names')
    last_at = 0
    for step in case['steps']:
        if not isinstance(step, dict):
            raise ValueError('step must be an object')
        actions = set(step) & {'send', 'send_hex', 'expect', 'expect_hex', 'match', 'journal_hash', 'silence_ms', 'expect_close'}
        if len(actions) != 1 or set(step) - actions - {'at_ms', 'within_ms', 'path', 'stream', 'finish'}:
            raise ValueError('step requires exactly one supported action')
        action = next(iter(actions))
        if 'stream' in step or 'finish' in step:
            if action not in ('send', 'send_hex'):
                raise ValueError('stream/finish only apply to sends')
            if type(step.get('stream')) is not int or not 0 <= step['stream'] < 2**63:
                raise ValueError('explicit stream must be uint63')
            if type(step.get('finish', True)) is not bool:
                raise ValueError('finish must be boolean')
        if 'path' in step and action != 'journal_hash':
            raise ValueError('path is only valid for journal_hash')
        at = millis(step.get('at_ms', last_at * 1000))
        if at < last_at:
            raise ValueError('at_ms must be nondecreasing')
        last_at = at
        millis(step.get('within_ms', 1000))
        if action in ('send', 'expect'):
            encode(step[action])
        if action in ('send_hex', 'expect_hex'):
            bytes.fromhex(step[action])
        if action == 'match':
            if not isinstance(step[action], dict) or not step[action]:
                raise ValueError('match must be nonempty dotted-path/value map')
            for key, value in step[action].items():
                if not isinstance(key, str) or not key:
                    raise ValueError('match paths must be nonempty text')
                encode(value)
        if action == 'journal_hash' and not re.fullmatch(r'[0-9a-f]{64}', step[action]):
            raise ValueError('journal_hash must be lowercase 256-bit hex')
        if action == 'silence_ms':
            millis(step[action])
        if action == 'expect_close' and step[action] is not True:
            raise ValueError('expect_close must be true')


def cases_for(root, suite):
    if not NAME.fullmatch(suite):
        raise ValueError(f'invalid suite name {suite!r}')
    files = sorted((root / suite).glob('**/*.json'))
    result = []
    names = set()
    for path in files:
        data = load_json(path)
        for case in data if isinstance(data, list) else [data]:
            validate_case(case)
            if case['name'] in names:
                raise ValueError(f'duplicate case name {suite}/{case["name"]}')
            names.add(case['name'])
            result.append(case)
    if not result:
        raise ValueError(f'claimed suite {suite!r} has no executable cases')
    return result


def validate_impl(impl):
    allowed = {'name', 'suites', 'transport', 'command', 'socket', 'framing',
               'endpoint', 'alpn', 'address', 'driver', 'connect_timeout_ms', 'fixtures', 'peer_roles', 'key_file', 'ready_line', 'fixture_clock', 'fixture_scheduler',
               'contract_profile'}
    if not isinstance(impl, dict) or set(impl) - allowed:
        raise ValueError('unknown implementation fields')
    if not isinstance(impl.get('name'), str) or not NAME.fullmatch(impl['name']):
        raise ValueError('invalid implementation name')
    suites = impl.get('suites')
    if not isinstance(suites, list) or not suites or any(
        not isinstance(s, str) or not NAME.fullmatch(s) for s in suites
    ) or len(set(suites)) != len(suites):
        raise ValueError('suites must be a nonempty unique name array')
    transport = impl.get('transport')
    if transport not in ('stdio', 'unix', 'iroh'):
        raise ValueError('transport must be stdio, unix or iroh')
    if impl.get('framing', 'u32be') not in ('u32be', 'cbor-sequence'):
        raise ValueError('unsupported framing')
    if transport == 'stdio':
        cmd = impl.get('command')
        if not isinstance(cmd, list) or not cmd or any(not isinstance(a, str) or not a for a in cmd):
            raise ValueError('command must be a nonempty argv array')
    if transport == 'unix' and not isinstance(impl.get('socket'), str):
        raise ValueError('unix transport needs socket')
    if transport == 'iroh':
        if not re.fullmatch('[0-9a-f]{64}', impl.get('endpoint', '')) or not impl.get('alpn'):
            raise ValueError('iroh requires endpoint hex and alpn')
    millis(impl.get('connect_timeout_ms', 5000))
    if profile_value(impl.get('contract_profile', DEFAULT_PROFILE), 'contract_profile') != DEFAULT_PROFILE and transport != 'stdio':
        # Provisioning is launch-time env; an already-running unix/iroh peer cannot receive it.
        raise ValueError('non-default contract_profile requires stdio transport')
    if 'fixture_clock' in impl and (type(impl['fixture_clock']) is not bool
                                      or transport != 'stdio'):
        raise ValueError('fixture_clock requires stdio and a boolean')
    if 'fixture_scheduler' in impl and (type(impl['fixture_scheduler']) is not bool
                                       or not impl.get('fixture_clock')):
        raise ValueError('fixture_scheduler requires fixture_clock and a boolean')
    if 'ready_line' in impl:
        line = impl['ready_line']
        if (transport != 'stdio' or not isinstance(line, str)
                or not re.fullmatch(r'[\x20-\x7e]{1,128}', line)):
            raise ValueError('ready_line requires stdio and 1..128 printable ASCII characters')
    roles = impl.get('peer_roles', ['runtime'])
    if not isinstance(roles, list) or not roles or any(not isinstance(s, str) or not NAME.fullmatch(s) for s in roles):
        raise ValueError('peer_roles must be a nonempty role array')
    fixtures = impl.get('fixtures', {})
    if not isinstance(fixtures, dict):
        raise ValueError('fixtures must be a label map')
    for label, hook in fixtures.items():
        if not LABEL.fullmatch(label) or not isinstance(hook, dict) or set(hook) - {'setup', 'teardown', 'env'}:
            raise ValueError('invalid fixture hook')
        for key in ('setup', 'teardown'):
            command = hook.get(key, [])
            if not isinstance(command, list) or any(not isinstance(a, str) or not a for a in command):
                raise ValueError('fixture hook commands must be argv arrays')
        env = hook.get('env', {})
        if isinstance(env, dict) and 'COGLET_FIXTURE_CLOCK_SOCKET' in env:
            raise ValueError('clock socket is harness-owned, not a fixture env setting')
        if not isinstance(env, dict) or any(not re.fullmatch(r'COGLET_FIXTURE_[A-Z0-9_]+', k) or not isinstance(v, str) for k, v in env.items()):
            raise ValueError('fixture env requires COGLET_FIXTURE_* string values')


def select_profiles(implementations, override):
    """Apply the optional CLI selector; it must agree with every selected registry entry."""
    if override is not None:
        profile_value(override, '--contract-profile')
        for impl in implementations:
            have = impl.get('contract_profile', DEFAULT_PROFILE)
            if have != override:
                raise ValueError(f'--contract-profile {override} conflicts with implementation '
                                 f'{impl["name"]} contract_profile {have}')
    return implementations


def check_case_profile(impl, suite, case):
    want = case.get('contract_profile', DEFAULT_PROFILE)
    have = impl.get('contract_profile', DEFAULT_PROFILE)
    if want != have:
        raise ValueError(f'{impl["name"]}.{suite}/{case["name"]} requires {want} but implementation '
                         f'is provisioned {have}')


def quarantined_ids(path):
    """[[conformance]] ids from ci/quarantine.toml (validated by scripts/ci/quarantine.py)."""
    if path is None:
        return set()
    entries = tomllib.loads(Path(path).read_text()).get('conformance', [])
    ids = {entry.get('id') for entry in entries if isinstance(entry, dict)}
    if not all(isinstance(ident, str) and ident for ident in ids):
        raise ValueError(f'{path}: every [[conformance]] entry needs a string id')
    return ids


def registry(path):
    data = tomllib.loads(path.read_text())
    if set(data) != {'implementations'} or not isinstance(data['implementations'], list):
        raise ValueError('registry requires implementations array')
    names = set()
    for impl in data['implementations']:
        validate_impl(impl)
        if impl['name'] in names:
            raise ValueError('duplicate implementation name')
        names.add(impl['name'])
    return data['implementations']


class BufferedOutput:
    """Drain clock-bound stdout while dispatch/flush barriers are outstanding.

    Retain at most 4 MiB of unread wire bytes. Overflow is a hard case failure,
    not backpressure: continue discarding so the child can flush and acknowledge.
    Framing, decoding and silence assertions still consume the original bytes.
    """
    LIMIT = 4 * 1024 * 1024

    def __init__(self, source):
        self.source = source
        self.buffer = bytearray()
        self.changed = asyncio.Event()
        self.eof = False
        self.error = None
        self.received = 0
        self.task = asyncio.create_task(self.drain())

    async def drain(self):
        try:
            while chunk := await self.source.read(65536):
                self.received += len(chunk)
                if self.error is None:
                    if len(self.buffer) + len(chunk) > self.LIMIT:
                        self.error = ValueError('fixture stdout exceeds 4 MiB unread byte limit')
                        self.buffer.clear()
                    else:
                        self.buffer.extend(chunk)
                self.changed.set()
        except Exception as error:
            self.error = error
        finally:
            self.eof = True
            self.changed.set()

    async def wait_received(self, total):
        while True:
            if self.error is not None:
                raise self.error
            if self.received >= total:
                return
            if self.eof:
                raise EOFError('fixture output ended before byte watermark')
            self.changed.clear()
            await self.changed.wait()

    async def readexactly(self, size):
        if not 0 <= size <= self.LIMIT:
            raise ValueError('fixture stdout read exceeds buffer limit')
        while True:
            if self.error is not None:
                raise self.error
            if len(self.buffer) >= size:
                result = bytes(self.buffer[:size])
                del self.buffer[:size]
                return result
            if self.eof:
                partial = bytes(self.buffer)
                self.buffer.clear()
                raise asyncio.IncompleteReadError(partial, size)
            self.changed.clear()
            await self.changed.wait()


class Peer:
    def __init__(self, impl, env=None):
        self.impl = impl
        self.env = env
        self.process = None
        self.writer = None
        self.stderr = bytearray()
        self.stderr_task = None
        self.ready = None
        self.clock_reader = None
        self.clock_writer = None
        self.clock_server = None
        self.clock_dir = None
        self.fixture_time = 0
        self.fixture_bytes = 0

    async def advance_clock(self, micros):
        if self.clock_writer is None:
            return
        if micros < self.fixture_time:
            raise ValueError('retrograde fixture clock')
        if self.impl.get('fixture_scheduler'):
            await self.schedule('advance', micros)
            return
        self.clock_writer.write(micros.to_bytes(8, 'big'))
        await self.clock_writer.drain()
        await self.clock_ack(micros)
        self.fixture_time = micros

    async def clock_ack(self, micros=None):
        if self.clock_reader is None:
            return
        if self.impl.get('fixture_scheduler'):
            await self.scheduler_ack(self.fixture_time, self.fixture_time)
            return
        expected = self.fixture_time if micros is None else micros
        raw = await self.clock_reader.readexactly(8)
        if int.from_bytes(raw, 'big') != expected:
            raise ValueError('fixture clock acknowledgement mismatch')
        if isinstance(self.reader, BufferedOutput) and self.reader.error is not None:
            raise self.reader.error

    async def scheduler_ack(self, minimum, maximum):
        assert self.clock_reader is not None and isinstance(self.reader, BufferedOutput)
        raw = await self.clock_reader.readline()
        if len(raw) > 256 or not raw.endswith(b'\n'):
            raise ValueError('invalid scheduler acknowledgement framing')
        ack = json.loads(raw)
        if (not isinstance(ack, dict) or set(ack) != {'time', 'bytes'}
                or type(ack['time']) is not int or type(ack['bytes']) is not int
                or not minimum <= ack['time'] <= maximum
                or ack['bytes'] < self.fixture_bytes):
            raise ValueError('invalid scheduler acknowledgement')
        await self.reader.wait_received(ack['bytes'])
        self.fixture_time, self.fixture_bytes = ack['time'], ack['bytes']

    async def schedule(self, operation, target):
        assert self.clock_writer is not None and isinstance(self.reader, BufferedOutput)
        previous = self.fixture_time
        self.clock_writer.write(json.dumps({operation: target}).encode() + b'\n')
        await self.clock_writer.drain()
        await self.scheduler_ack(target if operation == 'advance' else previous, target)
        if operation == 'next' and self.fixture_time <= previous and not self.reader.buffer:
            raise ValueError('fixture scheduler made no progress')

    async def scheduled_receive(self, within_ms):
        assert isinstance(self.reader, BufferedOutput)
        deadline = self.fixture_time + round(millis(within_ms) * 1_000_000)
        while not self.reader.buffer:
            if self.reader.eof:
                raise EOFError('fixture output ended')
            if self.fixture_time >= deadline:
                raise TimeoutError('logical fixture receive deadline')
            await self.schedule('next', deadline)
        if isinstance(self.reader, BufferedOutput) and self.reader.error is not None:
            raise self.reader.error

    async def start(self):
        impl = self.impl
        connected = None
        self.env = dict(self.env or os.environ)
        self.env.pop('COGLET_FIXTURE_CLOCK_SOCKET', None)
        if impl.get('fixture_clock'):
            self.clock_dir = tempfile.TemporaryDirectory(prefix='coglet-clock-')
            path = str(Path(self.clock_dir.name) / 'clock.sock')
            connected = asyncio.get_running_loop().create_future()
            def accept(reader, writer):
                if connected.done():
                    writer.close()
                else:
                    self.clock_reader, self.clock_writer = reader, writer
                    connected.set_result((reader, writer))
            self.clock_server = await asyncio.start_unix_server(accept, path=path)
            self.env = dict(self.env or os.environ, COGLET_FIXTURE_CLOCK_SOCKET=path)
        if impl['transport'] == 'unix':
            self.reader, self.writer = await asyncio.open_unix_connection(impl['socket'])
            return
        command = impl.get('command')
        if impl['transport'] == 'iroh':
            command = [impl.get('driver', str(SUITES / 'iroh-driver/target/debug/iroh-driver')),
                       '--endpoint', impl['endpoint'], '--alpn', impl['alpn']]
            if 'address' in impl:
                command += ['--address', impl['address']]
            if 'key_file' in impl:
                command += ['--key-file', impl['key_file']]
        self.process = await asyncio.create_subprocess_exec(
            *command, cwd=ROOT, stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
            start_new_session=True, env=self.env)
        self.reader, self.writer = self.process.stdout, self.process.stdin
        if impl.get('fixture_clock'):
            # Start before waiting for clock connect/readiness: initial frames
            # may themselves exceed pipe capacity before the child connects.
            self.reader = BufferedOutput(self.process.stdout)
        if 'ready_line' in impl:
            self.ready = asyncio.get_running_loop().create_future()
        self.stderr_task = asyncio.create_task(self.drain_stderr())
        if impl.get('fixture_clock'):
            assert connected is not None
            exited = asyncio.create_task(self.process.wait())
            try:
                await asyncio.wait((connected, exited), return_when=asyncio.FIRST_COMPLETED)
                if not connected.done():
                    raise RuntimeError('adapter exited before fixture clock connection')
                self.clock_reader, self.clock_writer = connected.result()
            finally:
                exited.cancel()
                await asyncio.gather(exited, return_exceptions=True)
        if self.ready is not None:
            await self.ready

    async def drain_stderr(self):
        assert self.process is not None and self.process.stderr is not None
        pending = b''
        marker = self.impl.get('ready_line', '').encode()
        while chunk := await self.process.stderr.read(4096):
            self.stderr.extend(chunk)
            del self.stderr[:-8192]
            if self.ready is not None and not self.ready.done():
                lines = (pending + chunk).split(b'\n')
                if marker in lines[:-1]:
                    self.ready.set_result(None)
                # Bound memory, retaining an overflow sentinel so a long line's
                # suffix cannot be mistaken for an exact readiness marker.
                pending = lines[-1]
                if len(pending) > 128:
                    pending = b'\x00' + pending[-128:]
        if self.ready is not None and not self.ready.done():
            self.ready.set_exception(RuntimeError('adapter exited before readiness signal'))

    async def close(self):
        if self.clock_server is not None:
            self.clock_server.close()
        if self.clock_writer is not None:
            self.clock_writer.close()
            try:
                await asyncio.wait_for(self.clock_writer.wait_closed(), .5)
            except (OSError, asyncio.TimeoutError):
                pass
        if self.clock_server is not None:
            try:
                await asyncio.wait_for(self.clock_server.wait_closed(), .5)
            except (OSError, asyncio.TimeoutError):
                pass
        if self.clock_dir is not None:
            try:
                self.clock_dir.cleanup()
            except OSError:
                pass
        if self.writer:
            self.writer.close()
        if self.process:
            assert self.reader is not None
            source = self.process.stdout
            assert source is not None
            async def discard_stdout():
                while await source.read(4096):
                    pass
            if isinstance(self.reader, BufferedOutput):
                drain = self.reader.task
            else:
                drain = asyncio.create_task(discard_stdout())
            # Kill the whole isolated session, including grandchildren on failure.
            try:
                os.killpg(self.process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                await asyncio.wait_for(self.process.wait(), .5)
            except asyncio.TimeoutError:
                pass
            try:
                os.killpg(self.process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            await self.process.wait()
            await asyncio.wait_for(drain, .5)
        if self.stderr_task:
            self.stderr_task.cancel()
            await asyncio.gather(self.stderr_task, return_exceptions=True)
        if self.ready is not None and self.ready.done() and not self.ready.cancelled():
            self.ready.exception()  # consume an early failure even before clock connect
        if self.writer:
            try:
                await asyncio.wait_for(self.writer.wait_closed(), .5)
            except (OSError, asyncio.TimeoutError):
                pass


def lookup(value, path):
    for key in path.split('.'):
        value = value[int(key)] if isinstance(value, list) else value[key]
    return value


async def timeline(peer, case):
    start = time.monotonic()
    framing = 'u32be' if peer.impl['transport'] == 'iroh' else peer.impl.get('framing', 'u32be')
    for index, step in enumerate(case['steps']):
        fixture_clock = peer.impl.get('fixture_clock', False)
        if not fixture_clock:
            await asyncio.sleep(max(0, start + millis(step.get('at_ms', 0)) - time.monotonic()))
        try:
            # ADR-0021: infrastructure liveness is not protocol fixture time.
            timeout = 3.0 if fixture_clock else millis(step.get('within_ms', 1000))
            if fixture_clock:
                target = max(peer.fixture_time, round(millis(step.get('at_ms', 0)) * 1_000_000))
                await asyncio.wait_for(peer.advance_clock(target), timeout)
            if 'send' in step or 'send_hex' in step:
                if peer.impl['transport'] != 'iroh' and ('stream' in step or 'finish' in step):
                    raise ValueError('stream/finish controls require iroh transport')
                raw = encode(step['send']) if 'send' in step else bytes.fromhex(step['send_hex'])
                if peer.impl['transport'] == 'iroh':
                    # Private driver control prefix, never sent over QUIC. Auto
                    # stream ids occupy the upper half, separate from explicit ids.
                    stream = step.get('stream', 2**63 + index)
                    raw = stream.to_bytes(8, 'big') + bytes([step.get('finish', True)]) + raw
                await asyncio.wait_for(write_frame(peer.writer, raw, framing), timeout)
                if fixture_clock:
                    await asyncio.wait_for(peer.clock_ack(), timeout)
                continue
            if 'expect_close' in step:
                # A timeout is not EOF, and even one unexpected byte (including
                # a truncated frame prefix) must not count as clean closure.
                try:
                    await asyncio.wait_for(peer.reader.readexactly(1), timeout)
                except asyncio.IncompleteReadError as error:
                    if error.partial:
                        raise AssertionError('unexpected bytes before close') from error
                    # A crashed stdio adapter is not evidence that its production
                    # peer deliberately closed. A bridge must end cleanly too.
                    process = getattr(peer, 'process', None)
                    if process is not None:
                        status = await asyncio.wait_for(process.wait(), timeout)
                        if status != 0:
                            raise AssertionError(f'peer exited with status {status} before close')
                    continue
                raise AssertionError('unexpected bytes before close')
            if 'silence_ms' in step:
                if fixture_clock:
                    target = peer.fixture_time + round(millis(step['silence_ms']) * 1_000_000)
                    await asyncio.wait_for(peer.advance_clock(target), timeout)
                if peer.impl.get('fixture_scheduler'):
                    if peer.reader.eof:
                        raise EOFError('fixture output ended during silence')
                    if peer.reader.buffer:
                        raise AssertionError('unexpected bytes during silence')
                    # A satisfied byte watermark does not synchronize EOF on
                    # the other channel. Retain the ordinary bounded wire probe
                    # as well; logical time stays frozen during this wait.
                try:
                    await asyncio.wait_for(peer.reader.readexactly(1), millis(step['silence_ms']))
                except asyncio.TimeoutError:
                    continue
                raise AssertionError('unexpected bytes during silence')
            if peer.impl.get('fixture_scheduler'):
                await asyncio.wait_for(peer.scheduled_receive(step.get('within_ms', 1000)), timeout)
            raw = await asyncio.wait_for(read_frame(peer.reader, framing), timeout)
            if 'expect_hex' in step:
                if raw != bytes.fromhex(step['expect_hex']):
                    raise AssertionError(f'wire mismatch: {raw.hex()}')
                continue
            value = decode(raw)
            if 'expect' in step and encode(value) != encode(step['expect']):
                raise AssertionError(f'expected {step["expect"]!r}, received {value!r}')
            for path, expected in step.get('match', {}).items():
                if encode(lookup(value, path)) != encode(expected):
                    raise AssertionError(f'predicate {path} != {expected!r}: {value!r}')
            if 'journal_hash' in step:
                if lookup(value, step.get('path', 'state_hash')) != step['journal_hash']:
                    raise AssertionError(f'journal hash mismatch: {value!r}')
        except Exception as error:
            raise AssertionError(f'step {index}: {type(error).__name__}: {error}') from error


async def run_hook(command, env):
    if not command:
        return
    # Hook stdout is discarded; configuration is explicit in fixture env.
    peer = Peer(dict(transport='stdio', command=command), env)
    try:
        await peer.start()
        assert peer.writer is not None and peer.reader is not None and peer.process is not None
        peer.writer.close()
        source = peer.process.stdout
        assert source is not None
        async def discard():
            while await source.read(4096):
                pass
        async with asyncio.timeout(10):
            await asyncio.gather(discard(), peer.process.wait())
        if peer.process.returncode:
            raise RuntimeError(f'fixture hook exited {peer.process.returncode}: {peer.stderr.decode(errors="replace")}')
    finally:
        await peer.close()


async def run_case(impl, suite, case):
    labels = case.get('fixtures', [])
    hooks = impl.get('fixtures', {})
    missing = [label for label in labels if label not in hooks]
    skipped = 'fixture unsupported: ' + ', '.join(missing) if missing else None
    if case.get('peer_role', 'runtime') not in impl.get('peer_roles', ['runtime']):
        skipped = 'peer_role unsupported: ' + case.get('peer_role', 'runtime')
    if skipped:
        return (f'{impl["name"]}.{suite}', case['name'], 0, None, skipped)
    env = dict(os.environ, COGLET_CONFORMANCE_CASE=case['name'],
               COGLET_CONFORMANCE_SUITE=suite, COGLET_CONFORMANCE_PEER_ROLE=case.get('peer_role', 'runtime'))
    # Always set (overriding any inherited value) so omission is an explicit 0.1.
    env[PROFILE_ENV] = impl.get('contract_profile', DEFAULT_PROFILE)
    for label in labels:
        env.update(hooks[label].get('env', {}))
    peer = Peer(impl, env)
    started = time.monotonic()
    error = None
    entered = []
    try:
        async with asyncio.timeout(millis(case.get('timeout_ms', 10000))):
            for label in labels:
                entered.append(label)
                await run_hook(hooks[label].get('setup', []), env)
            await asyncio.wait_for(peer.start(), millis(impl.get('connect_timeout_ms', 5000)))
            await timeline(peer, case)
    except Exception as exc:
        error = f'{type(exc).__name__}: {exc}'
    finally:
        await peer.close()
        for label in reversed(entered):
            try:
                await run_hook(hooks[label].get('teardown', []), env)
            except Exception as exc:
                error = (error + '\n' if error else '') + f'teardown {label}: {exc}'
    if error and peer.stderr:
        error += '\nstderr: ' + peer.stderr.decode(errors='replace')
    return (f'{impl["name"]}.{suite}', case['name'], time.monotonic() - started, error, None)


def xml_text(text):
    return ''.join(c for c in text if c in '\t\n\r' or 32 <= ord(c) <= 0xd7ff or 0xe000 <= ord(c) <= 0xfffd or 0x10000 <= ord(c) <= 0x10ffff)


def report(path, results):
    suite = ET.Element('testsuite', name='coglet-conformance', tests=str(len(results)),
                       failures=str(sum(r[3] is not None for r in results)),
                       skipped=str(sum(r[4] is not None for r in results)),
                       time=f'{sum(r[2] for r in results):.6f}')
    for classname, name, elapsed, error, skipped in results:
        case = ET.SubElement(suite, 'testcase', classname=xml_text(classname),
                             name=xml_text(name), time=f'{elapsed:.6f}')
        if error is not None:
            ET.SubElement(case, 'failure', message=xml_text(error.splitlines()[0])).text = xml_text(error)
        if skipped is not None:
            ET.SubElement(case, 'skipped', message=xml_text(skipped))
    path.parent.mkdir(parents=True, exist_ok=True)
    ET.ElementTree(suite).write(path, encoding='utf-8', xml_declaration=True)


async def execute(args):
    results = []
    try:
        if args.self_test:
            implementations = [dict(name='loopback', suites=['self-test'], transport='stdio',
                                    command=[sys.executable, str(SUITES / 'loopback.py')])]
        elif args.impl or args.socket or args.endpoint:
            if not args.suite:
                raise ValueError('direct implementation requires --suite')
            impl: dict = dict(name='direct', suites=args.suite)
            impl['peer_roles'] = args.peer_role or ['runtime']
            if args.impl:
                impl.update(transport='stdio', command=shlex.split(args.impl))
            elif args.socket:
                impl.update(transport='unix', socket=args.socket)
            else:
                impl.update(transport='iroh', endpoint=args.endpoint, alpn=args.alpn)
                if args.address:
                    impl['address'] = args.address
                if args.driver:
                    impl['driver'] = args.driver
                if args.key_file:
                    impl['key_file'] = args.key_file
            impl['framing'] = args.framing
            if args.contract_profile is not None:
                impl['contract_profile'] = args.contract_profile
            validate_impl(impl)
            implementations = [impl]
        else:
            implementations = registry(args.registry)
        select_profiles(implementations, args.contract_profile)
        # Preflight all claims before launching anything, never silently skip a suite.
        jobs = [(impl, suite, case) for impl in implementations for suite in impl['suites']
                for case in cases_for(args.cases, suite)]
        for impl, suite, case in jobs:
            check_case_profile(impl, suite, case)  # before any launch
        if not jobs:
            print('No implementations registered; no conformance claims made.')
        quarantined = quarantined_ids(args.quarantine)
        if args.only_quarantined and args.quarantine is None:
            raise ValueError('--only-quarantined requires --quarantine')
        keys = {f'{impl["name"]}.{suite}/{case["name"]}' for impl, suite, case in jobs}
        for ident in sorted(quarantined - keys):
            print(f'QUARANTINE entry matches no case in this run: {ident}', file=sys.stderr)
        for impl, suite, case in jobs:
            key = f'{impl["name"]}.{suite}/{case["name"]}'
            if args.only_quarantined and key not in quarantined:
                continue
            if key in quarantined and not args.only_quarantined:
                result = (f'{impl["name"]}.{suite}', case['name'], 0, None, 'quarantined (ci/quarantine.toml)')
            else:
                result = await run_case(impl, suite, case)
            results.append(result)
            print(f'{"SKIP" if result[4] else "FAIL" if result[3] else "PASS"} {result[0]}/{result[1]}' + (f': {result[4]}' if result[4] else ''))
            if result[3]:
                print(result[3], file=sys.stderr)
    except Exception as error:
        results.append(('harness', 'configuration', 0, f'{type(error).__name__}: {error}', None))
        print(results[-1][3], file=sys.stderr)
    report(args.junit, results)
    return int(any(r[3] is not None for r in results))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument('--self-test', action='store_true')
    selection.add_argument('--impl', help='shell-quoted argv, executed without a shell')
    selection.add_argument('--socket', help='existing local Unix socket')
    selection.add_argument('--endpoint', help='iroh endpoint public key (hex)')
    parser.add_argument('--suite', action='append')
    parser.add_argument('--peer-role', action='append', help='supported harness role for direct mode (repeatable)')
    parser.add_argument('--alpn')
    parser.add_argument('--address', help='optional direct iroh IP:port')
    parser.add_argument('--driver')
    parser.add_argument('--key-file', help='iroh fixture secret key file, never key material')
    parser.add_argument('--contract-profile', choices=PROFILES, default=None,
                        help='trusted local contract profile (default surface-0.1); must agree with every '
                             'selected registry entry contract_profile; passed via ' + PROFILE_ENV
                             + ', never on the wire')
    parser.add_argument('--framing', choices=['u32be', 'cbor-sequence'], default='u32be')
    parser.add_argument('--registry', type=Path, default=SUITES / 'impls.toml')
    parser.add_argument('--cases', type=Path, default=SUITES)
    parser.add_argument('--junit', type=Path, default=ROOT / 'target/conformance-results.xml')
    parser.add_argument('--quarantine', type=Path,
                        help='skip the [[conformance]] ids listed in this file (ci/quarantine.toml)')
    parser.add_argument('--only-quarantined', action='store_true',
                        help='run only the quarantined cases (stability workflow)')
    return asyncio.run(execute(parser.parse_args()))
