#!/usr/bin/env python3
"""Non-opted-in adapter timing checks, separate from functional conformance.

Bounds below are runner watchdogs, not latency claims. No fixture clock is
connected; only the real adapter idle loop can produce health and local kill.
"""
import asyncio
import json
import os
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'conformance'))
from harness import Peer, read_frame, write_frame, encode, decode


async def check(binary, kill):
    env = dict(os.environ, COGLET_FIXTURE_CONFIG=str(ROOT / 'spec/fixtures/surface/adapter-config.json'),
               COGLET_CONFORMANCE_CASE='live-clock', COGLET_FIXTURE_KILL_AT_MS='100' if kill else '')
    peer = Peer(dict(transport='stdio', command=[binary], ready_line='COGLET_CONFORMANCE_READY'), env)
    async def receive():
        return decode(await asyncio.wait_for(read_frame(peer.reader, 'u32be'), 2.5))
    async def send(value):
        await write_frame(peer.writer, encode(value), 'u32be')
    async def expect(kind):
        value = await receive()
        assert value['type'] == kind, value
        return value['body']
    try:
        await asyncio.wait_for(peer.start(), 3)
        assert peer.clock_writer is None
        await expect('hello')
        case = json.loads((ROOT / 'conformance/surface/kill-latched.json').read_text())
        await send(case['steps'][1]['send'])
        await expect('capabilities')
        await expect('grants')
        if kill:
            # No inbound stimulus and no clock advances: local kill must run idle.
            await expect('kill')
            health = await expect('health')
            assert health['state'] == 'stopped' and health['clock'] >= 100_000, health
            await send(case['steps'][6]['send'])
            reply = await expect('accept')
            assert not reply['accepted'] and reply['error']['code'] == 'permission_denied', reply
        else:
            # Observe actual elapsed time before submitting a positive, now-expired
            # deadline. A frozen wall-clock fallback cannot satisfy either assertion.
            health = await expect('health')
            assert health['state'] == 'ready' and health['clock'] >= 1_000_000, health
            action = case['steps'][6]['send']
            action['body']['action']['deadline'] = 500_000
            await send(action)
            reply = await expect('accept')
            assert not reply['accepted'] and reply['error']['code'] == 'deadline_exceeded', reply
        print('PASS live-clock ' + ('independent-kill' if kill else 'idle-health-and-deadline-expiry'))
    finally:
        await peer.close()


async def main():
    for kill in (False, True):
        await check(str(Path(sys.argv[1]).resolve()), kill)


if __name__ == '__main__':
    asyncio.run(main())
