#!/usr/bin/env python3
"""20 full Swift registry passes under two owned CPU burners; retain every result."""
import asyncio
import json
import os
from pathlib import Path
import platform
import resource
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'conformance'))
from harness import SUITES, cases_for, registry, report, run_case


async def run(output):
    output.mkdir(parents=True, exist_ok=True)
    impls = registry(ROOT / 'sdk/surface-swift/conformance.toml')
    assert len(impls) == 1
    cases = cases_for(SUITES, 'surface')
    assert len(cases) == 11
    workers = []
    results = []
    started = time.monotonic()
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    try:
        for _ in range(2):
            worker = await asyncio.create_subprocess_exec(
                sys.executable, __file__, '--cpu-worker', stdout=asyncio.subprocess.PIPE)
            workers.append(worker)
            assert await asyncio.wait_for(worker.stdout.readline(), 5) == b'CPU_READY\n'
        for iteration in range(1, 21):
            if any(worker.returncode is not None for worker in workers):
                raise RuntimeError('CPU load exited during proof')
            batch = [await run_case(impls[0], 'surface', case) for case in cases]
            report(output / f'iteration-{iteration:02}.xml', batch)
            results.extend(batch)
            print(json.dumps(dict(iteration=iteration, results=batch)), flush=True)
            if any(row[3] for row in batch):
                raise RuntimeError(f'iteration {iteration} failed; not consecutive passes')
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
                   passes=sum(r[3] is None and r[4] is None for r in results),
                   failures=sum(r[3] is not None for r in results),
                   role_skips=sum(r[4] is not None for r in results),
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
    asyncio.run(run(Path('target/swift-readiness')))
