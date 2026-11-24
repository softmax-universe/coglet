#!/usr/bin/env python3
"""Same frozen router cases/oracles against plain Kernel and MockHub.

Exit 2 means incomplete, even when every implemented case passed. No acceptance
case is removed from the default run and unsupported fixture labels stay SKIP.
"""
import argparse
import asyncio
import time
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
SUPPORTED = (
    'enroll-reconnect', 'health-minimal', 'enroll-unsupported-version',
    'enroll-one-use', 'enroll-expired', 'enroll-window-closed',
    'enroll-wrong-node', 'start-before-enroll',
    'event-batch-dedup', 'event-offset-conflict', 'event-source-repeat',
    'event-empty-probe', 'event-source-gap', 'event-retry-a-after-b',
    'event-gap', 'event-runtime-wake', 'event-coglet-forged-event', 'event-foreign-coglet',
    *(f'fetch-{kind}-{mode}' for kind in ('published', 'logs')
      for mode in ('catch-up', 'identical', 'equal-conflict', 'older')),
)


FETCH = {name for name in SUPPORTED if name.startswith('fetch-')}


async def run_fetch(backend, case, key):
    """Run unchanged steps through the real driver, then await durable oracle.

    The only test-local trigger is after enrollment's actual reply match. Neither
    operation frames nor expected results are generated or translated here.
    """
    from harness import Peer, timeline, validate_impl
    started = time.monotonic()
    server = None
    peer = None
    error = None
    try:
        async with asyncio.timeout(case.get('timeout_ms', 10000) / 1000):
            env = dict(os.environ, COGLET_CONFORMANCE_CASE=case['name'],
                       COGLET_FIXTURE_IMPLEMENTATION=backend)
            env.pop('COGLET_FIXTURE_EXPECT_RESET_STREAM', None)
            if case['name'].endswith(('older', 'equal-conflict')):
                env['COGLET_FIXTURE_EXPECT_RESET_STREAM'] = '100'
            server = await asyncio.create_subprocess_exec(
                str(ROOT / 'target/debug/examples/router_conformance_peer'),
                cwd=ROOT, env=env, stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
            if server.stdout is None or server.stdin is None:
                raise RuntimeError('observer pipes unavailable')
            ready = json.loads(await server.stdout.readline())
            required = {f'hub.{case["name"]}', 'hub.reverse-fetch-observer'}
            if set(case.get('fixtures', [])) != required or case.get('peer_role') != 'device':
                raise ValueError('fetch fixture/role mapping drift')
            impl = dict(name=f'router-{backend}', suites=['hub'], peer_roles=['device'],
                        fixtures={label: {} for label in required},
                        transport='iroh', endpoint=ready['endpoint'], address=ready['address'],
                        alpn='coglet/hub/0', key_file=str(key), incoming_stream=100,
                        driver=str(ROOT / 'conformance/iroh-driver/target/debug/iroh-driver'))
            validate_impl(impl)
            peer = Peer(impl, env)
            await peer.start()
            if (case['steps'][0]['send']['method'] != 'hub.enroll'
                    or case['steps'][1]['match']['type'] != 'reply'):
                raise ValueError('fetch enrollment trigger mapping drift')
            await timeline(peer, dict(case, steps=case['steps'][:2]))
            server.stdin.write(b'G')
            await server.stdin.drain()
            await timeline(peer, dict(case, steps=case['steps'][2:]))
            await peer.finish_wire()
            server.stdin.write(b'D')
            await server.stdin.drain()
            extra, diagnostic = await server.communicate()
            if server.returncode or extra:
                raise RuntimeError(f'observer oracle exit {server.returncode}: {diagnostic.decode()} {extra!r}')
    except Exception as exc:
        error = f'{type(exc).__name__}: {exc}'
    finally:
        if peer is not None:
            await peer.close()
            if error and peer.stderr:
                error += '\n' + peer.stderr.decode(errors='replace')
        if server is not None and server.returncode is None:
            server.kill()
            _, diagnostic = await server.communicate()
            if error:
                error += '\n' + diagnostic.decode(errors='replace')
    result = (f'router-{backend}.hub', case['name'], time.monotonic() - started, error, None)
    print(('FAIL' if error else 'PASS'), result[0], result[1], error or '')
    return result


async def exercise(cases, key):
    from harness import run_case, validate_impl
    results = []
    for backend in ('reference', 'mock'):
        impl = dict(name=f'router-{backend}', suites=['hub'], peer_roles=['device', 'hub'],
                    transport='stdio', command=['target/debug/examples/router_conformance_peer'],
                    finish_on_eof=True, ready_line='COGLET_ROUTER_READY',
                    fixtures={f'hub.{name}': {'env': {'COGLET_FIXTURE_IMPLEMENTATION': backend}}
                              for name in SUPPORTED if name not in FETCH})
        impl['fixtures']['hub.reverse-stream'] = {}
        validate_impl(impl)
        for case in cases:
            if case['name'] in FETCH:
                results.append(await run_fetch(backend, case, key))
            else:
                results.append(await run_case(impl, 'hub', case))
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--no-build', action='store_true')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/router-conformance.xml')
    args = parser.parse_args()
    os.chdir(ROOT)
    if not args.no_build:
        subprocess.run(['cargo', 'build', '--locked', '-j2', '-p', 'coglet-proto',
                        '--bin', 'coglet-git-validator'], cwd=ROOT, check=True)
        subprocess.run(['cargo', 'build', '--locked', '-j2', '--manifest-path',
                        'conformance/iroh-driver/Cargo.toml'], cwd=ROOT, check=True)
        subprocess.run(['cargo', 'build', '--locked', '-j2', '-p', 'coglet-mock',
                        '--features', 'test-controls', '--example', 'router_conformance_peer'],
                       cwd=ROOT, check=True)
    sys.path.insert(0, str(ROOT / 'conformance'))
    from harness import cases_for
    cases = cases_for(ROOT / 'conformance', 'hub')
    names = {case['name'] for case in cases}
    if len(SUPPORTED) != len(set(SUPPORTED)) or set(SUPPORTED) - names:
        raise SystemExit('supported router fixture list is duplicated or stale')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.unlink(missing_ok=True)
    (ROOT / 'target').mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='router-conformance-',
                                     dir=os.environ.get('TMPDIR', ROOT / 'target')) as temporary:
        key = Path(temporary) / 'device.key'
        key.write_text('03' * 32)
        key.chmod(0o600)
        from harness import report
        results = asyncio.run(exercise(cases, key))
        report(args.output, results)
    report = ET.parse(args.output).getroot()
    entries = report.findall('testcase')
    expected = {(f'router-{backend}.hub', case['name'])
                for backend in ('reference', 'mock') for case in cases}
    actual = [(case.get('classname'), case.get('name')) for case in entries]
    if len(actual) != len(expected) or set(actual) != expected:
        raise SystemExit('router conformance lost/duplicated an acceptance case')
    for case in entries:
        supported = case.get('name') in SUPPORTED
        skipped = case.find('skipped') is not None
        if supported == skipped:
            raise SystemExit(f'fixture support drift: {case.get("classname")}/{case.get("name")}')
    summary = {}
    for backend in ('reference', 'mock'):
        scoped = [case for case in entries if case.get('classname') == f'router-{backend}.hub']
        summary[backend] = dict(total=len(scoped),
                               failed=sum(case.find('failure') is not None or case.find('error') is not None for case in scoped),
                               skipped=sum(case.find('skipped') is not None for case in scoped))
        summary[backend]['passed'] = (summary[backend]['total'] - summary[backend]['failed'] - summary[backend]['skipped'])
    print(json.dumps(summary, sort_keys=True))
    if any(s['failed'] for s in summary.values()):
        return 1
    if any(s['skipped'] for s in summary.values()):
        print('INCOMPLETE: full router conformance is not yet implemented.', file=sys.stderr)
        return 2
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
