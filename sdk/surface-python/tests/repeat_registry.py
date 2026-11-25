#!/usr/bin/env python3
"""20 consecutive complete Python functional registries with two owned burners."""
import asyncio
import json
import os
from pathlib import Path
import platform
import resource
import sys
import time

from test_conformance import registry_run


async def main(output):
    output.mkdir(parents=True, exist_ok=True)
    workers = []
    results = []
    started = time.monotonic()
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    try:
        for _ in range(2):
            worker = await asyncio.create_subprocess_exec(sys.executable, __file__, '--cpu-worker',
                stdout=asyncio.subprocess.PIPE)
            workers.append(worker)
            assert await asyncio.wait_for(worker.stdout.readline(), 5) == b'CPU_READY\n'
        for iteration in range(1, 21):
            assert all(worker.returncode is None for worker in workers), 'CPU worker exited'
            batch = await registry_run(output / f'iteration-{iteration:02}.xml')
            results.extend(batch)
            print(json.dumps(dict(iteration=iteration, results=batch)), flush=True)
            assert all(row[3] is None for row in batch), batch
    finally:
        for worker in workers:
            if worker.returncode is None:
                worker.terminate()
        for worker in workers:
            await asyncio.wait_for(worker.wait(), 5)
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    elapsed = time.monotonic() - started
    cpu = after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime
    summary = dict(iterations=20, results=len(results),
        passes=sum(row[3] is None and row[4] is None for row in results),
        failures=sum(row[3] is not None for row in results),
        role_skips=sum(row[4] is not None for row in results),
        elapsed_s=elapsed, child_cpu_s=cpu, cpu_workers=2,
        runner=os.environ.get('RUNNER_NAME'), platform=platform.platform(),
        head=os.environ.get('GITHUB_SHA'))
    (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps(summary), flush=True)
    assert summary['passes'] == 180 and summary['role_skips'] == 40
    assert cpu > elapsed * .25, 'no measured CPU load'


if __name__ == '__main__':
    if sys.argv[1:] == ['--cpu-worker']:
        print('CPU_READY', flush=True)
        while True:
            pass
    asyncio.run(main(Path('target/python-readiness')))
