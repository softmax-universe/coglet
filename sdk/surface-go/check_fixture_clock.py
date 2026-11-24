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
        await timeline(peer, dict(steps=case['steps'][:6]))
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
            assert error is not None and 'TimeoutError' in error, error
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
