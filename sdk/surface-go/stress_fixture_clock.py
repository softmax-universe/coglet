#!/usr/bin/env python3
"""Non-required stability proof: real adapter, uncached comparisons, owned load.

Pause the adapter after grants, before its local kill timer fires. This isolates
setup scheduling from the later 450ms stall that is the comparison's subject.
No protocol bytes or fixture clocks are mocked.
"""
import argparse
import asyncio
import os
import signal
import subprocess
import sys

import check_fixture_clock as comparison
import harness


async def run(args):
    original_read = harness.read_frame
    original_peer = comparison.Peer
    current = None
    resume = None

    class OwnedPeer(original_peer):
        async def start(self):
            nonlocal current
            current = self
            await super().start()

    async def read_frame(*values, **keywords):
        nonlocal resume
        raw = await original_read(*values, **keywords)
        if harness.decode(raw).get('type') == 'grants' and args.setup_stall_ms:
            pid = current.process.pid
            os.kill(pid, signal.SIGSTOP)
            resume = asyncio.get_running_loop().call_later(
                args.setup_stall_ms / 1000, os.kill, pid, signal.SIGCONT)
        return raw

    comparison.Peer = OwnedPeer
    harness.read_frame = read_frame
    try:
        for iteration in range(1, args.repeats + 1):
            for logical in (False, True):
                try:
                    await comparison.check(args.binary, logical)
                finally:
                    if resume:
                        resume.cancel()
                        resume = None
            print(f'PASS scheduler-stall iteration {iteration}/{args.repeats}', flush=True)
    finally:
        comparison.Peer = original_peer
        harness.read_frame = original_read


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary')
    parser.add_argument('--repeats', type=int, default=50)
    parser.add_argument('--setup-stall-ms', type=int, default=350)
    parser.add_argument('--load-workers', type=int, default=2)
    args = parser.parse_args()
    if args.repeats < 1 or args.setup_stall_ms < 0 or args.load_workers < 0:
        parser.error('positive repeats and nonnegative stall/load required')
    # Bound load to one CPU, shared with this harness and its own adapter children.
    # Never signal a name-matched or pre-existing process.
    os.sched_setaffinity(0, {min(os.sched_getaffinity(0))})
    workers = []
    try:
        for _ in range(args.load_workers):
            workers.append(subprocess.Popen([sys.executable, '-c', 'while True: pass']))
        asyncio.run(run(args))
    finally:
        for worker in workers:
            worker.terminate()
        for worker in workers:
            worker.wait(timeout=5)


if __name__ == '__main__':
    main()
