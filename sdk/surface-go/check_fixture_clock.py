"""Controlled scheduler-stall comparison, same unchanged kill-latched fixture."""
import asyncio
import copy
import json
import os
from pathlib import Path
import signal
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT/'conformance'))
from harness import Peer, timeline


async def check(binary, logical):
    impl = next(i for i in tomllib.loads((ROOT/'conformance/impls.toml').read_text())['implementations'] if i['name']=='surface-go-reference')
    assert impl['fixture_clock'] is True, 'Go registry must opt into ADR-0021'
    impl = copy.deepcopy(impl)
    impl['command'] = [str(Path(binary).resolve())]
    impl['fixture_clock'] = logical
    case = json.loads((ROOT/'conformance/surface/kill-latched.json').read_text())
    env = dict(os.environ, COGLET_CONFORMANCE_CASE=case['name'])
    for fixture in case['fixtures']:
        env.update(impl['fixtures'][fixture].get('env', {}))
    peer = Peer(impl, env)
    resume = None
    try:
        await asyncio.wait_for(peer.start(), 3)
        # This prefix establishes the killed state; it is not the latency
        # assertion under test. A wall-clock adapter may be descheduled between
        # grants and its local kill timer. Use ADR-0021's 3s infrastructure
        # watchdog here, as for start/fixture-clock I/O, not the fixture's 200ms
        # protocol budget. check_live_clock.py separately proves idle real-time
        # kill and clock progress. Never relax the unchanged suffix below: its
        # 200ms receive must still reject our later 450ms scheduling stall.
        setup = copy.deepcopy(case['steps'][:6])
        for step in setup:
            if 'within_ms' in step:
                step['within_ms'] = 3000
        await timeline(peer, dict(steps=setup))
        # Stop only our own exact subprocess. This deterministically models an
        # adapter scheduling gap, without mocking messages or durable storage.
        os.kill(peer.process.pid, signal.SIGSTOP)
        resume = asyncio.get_running_loop().call_later(.45, os.kill, peer.process.pid, signal.SIGCONT)
        error = None
        try:
            await timeline(peer, dict(steps=case['steps'][6:]))
        except AssertionError as exc:
            error = str(exc)
        print(json.dumps(dict(fixture_clock=logical, scheduler_stall_ms=450, error=error)), flush=True)
        if logical:
            assert error is None, error
        else:
            assert error is not None and error.startswith('step 1: TimeoutError:'), error
    finally:
        if resume:
            resume.cancel()
        if peer.process:
            try:
                os.kill(peer.process.pid, signal.SIGCONT)
            except ProcessLookupError:
                pass
        await peer.close()


async def main():
    await check(sys.argv[1], False)
    await check(sys.argv[1], True)
    print('PASS unchanged case: real-time budget fails under owned stall; logical fixture passes')


if __name__=='__main__':
    asyncio.run(main())
