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
import time
import tomllib
import xml.etree.ElementTree as ET

from wire import decode, encode, read_frame, write_frame

ROOT = Path(__file__).resolve().parent.parent
SUITES = ROOT / 'conformance'
NAME = re.compile(r'[a-z][a-z0-9_-]*\Z')
LABEL = re.compile(r'[a-z][a-z0-9_.-]*\Z')


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
    if not isinstance(case, dict) or set(case) - {'name', 'steps', 'timeout_ms', 'description', 'peer_role', 'fixtures'}:
        raise ValueError('case must have name, steps and optional timeout_ms/description')
    if not isinstance(case.get('name'), str) or not case['name']:
        raise ValueError('case name must be nonempty text')
    if not isinstance(case.get('steps'), list) or not case['steps']:
        raise ValueError('case steps must be a nonempty array')
    millis(case.get('timeout_ms', 10000))
    if not isinstance(case.get('peer_role', 'runtime'), str) or not NAME.fullmatch(case.get('peer_role', 'runtime')):
        raise ValueError('invalid peer_role')
    labels = case.get('fixtures', [])
    if not isinstance(labels, list) or any(not isinstance(s, str) or not LABEL.fullmatch(s) for s in labels) or len(set(labels)) != len(labels):
        raise ValueError('fixtures must be unique label names')
    last_at = 0
    for step in case['steps']:
        if not isinstance(step, dict):
            raise ValueError('step must be an object')
        actions = set(step) & {'send', 'send_hex', 'expect', 'expect_hex', 'match', 'journal_hash', 'silence_ms'}
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
               'endpoint', 'alpn', 'address', 'driver', 'connect_timeout_ms', 'fixtures', 'peer_roles', 'key_file', 'ready_line'}
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
        if not isinstance(env, dict) or any(not re.fullmatch(r'COGLET_FIXTURE_[A-Z0-9_]+', k) or not isinstance(v, str) for k, v in env.items()):
            raise ValueError('fixture env requires COGLET_FIXTURE_* string values')


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


class Peer:
    def __init__(self, impl, env=None):
        self.impl = impl
        self.env = env
        self.process = None
        self.writer = None
        self.stderr = bytearray()
        self.stderr_task = None
        self.ready = None

    async def start(self):
        impl = self.impl
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
        if 'ready_line' in impl:
            self.ready = asyncio.get_running_loop().create_future()
        self.stderr_task = asyncio.create_task(self.drain_stderr())
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
        if self.writer:
            self.writer.close()
        if self.process:
            assert self.reader is not None
            async def discard_stdout():
                while await self.reader.read(4096):
                    pass
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
        await asyncio.sleep(max(0, start + millis(step.get('at_ms', 0)) - time.monotonic()))
        try:
            timeout = millis(step.get('within_ms', 1000))
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
                continue
            if 'silence_ms' in step:
                try:
                    await asyncio.wait_for(peer.reader.readexactly(1), millis(step['silence_ms']))
                except asyncio.TimeoutError:
                    continue
                raise AssertionError('unexpected bytes during silence')
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
        async def discard():
            while await peer.reader.read(4096):
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
            validate_impl(impl)
            implementations = [impl]
        else:
            implementations = registry(args.registry)
        # Preflight all claims before launching anything, never silently skip a suite.
        jobs = [(impl, suite, case) for impl in implementations for suite in impl['suites']
                for case in cases_for(args.cases, suite)]
        if not jobs:
            print('No implementations registered; no conformance claims made.')
        for impl, suite, case in jobs:
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
    parser.add_argument('--framing', choices=['u32be', 'cbor-sequence'], default='u32be')
    parser.add_argument('--registry', type=Path, default=SUITES / 'impls.toml')
    parser.add_argument('--cases', type=Path, default=SUITES)
    parser.add_argument('--junit', type=Path, default=ROOT / 'target/conformance-results.xml')
    return asyncio.run(execute(parser.parse_args()))
