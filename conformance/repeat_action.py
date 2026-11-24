#!/usr/bin/env python3
"""Repeat first-action/dedup cases under owned single-core CPU contention.

Uniformly delay every action-path fdatasync, not a removed syscall ordinal.
The old binaries must reproduce step-5 timeouts; fixed binaries must pass with
unchanged case deadlines. This isolates redundant durability barriers, not the
unrecorded cause of any historical CI runner's disk/scheduler latency.
"""
import argparse
import asyncio
import copy
import json
import os
from pathlib import Path
import resource
import shutil
import subprocess
import sys
import time

from harness import SUITES, cases_for, registry, report, run_case

NAMES = {'action-accept-outcome', 'dedup-terminal'}


async def run(args):
    if not shutil.which('strace') or not hasattr(os, 'sched_setaffinity'):
        raise RuntimeError('proof requires Linux CPU affinity and strace')
    args.output.mkdir(parents=True, exist_ok=True)
    cpu = min(os.sched_getaffinity(0))
    os.sched_setaffinity(0, {cpu})
    workers = [subprocess.Popen([sys.executable, __file__, '--cpu-worker'])
               for _ in range(2)]
    results = []
    started = time.monotonic()
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    try:
        implementations = [copy.deepcopy(i) for i in registry(SUITES / 'impls.toml')
                           if i['name'] in ('surface-rust-reference', 'mock-surface',
                                            'surface-rust-reference-in-process',
                                            'mock-surface-in-process')]
        assert len(implementations) == 4
        cases = [c for c in cases_for(SUITES, 'surface') if c['name'] in NAMES]
        assert len(cases) == 2
        for iteration in range(1, args.iterations + 1):
            batch = []
            for impl in implementations:
                for case in cases:
                    traced = copy.deepcopy(impl)
                    if args.binary_dir:
                        traced['command'][0] = str(args.binary_dir / Path(impl['command'][0]).name)
                    trace = args.output.resolve() / f'{iteration:03}-{impl["name"]}-{case["name"]}.trace'
                    # Calls 1 and 2 are Init and grant epoch. Delay ALL later
                    # syncs identically, including additional future syncs.
                    traced['command'] = [
                        'strace', '-f', '-qq', '-tt', '-T', '-e', 'trace=fdatasync,write',
                        '-e', f'inject=fdatasync:delay_enter={args.sync_delay_ms}ms:when=3+',
                        '-o', str(trace), *traced['command']]
                    batch.append(await run_case(traced, 'surface', case))
            report(args.output / f'iteration-{iteration:03}.xml', batch)
            results.extend(batch)
            print(json.dumps(dict(iteration=iteration, results=batch)), flush=True)
    finally:
        for worker in workers:
            worker.terminate()
        for worker in workers:
            worker.wait()
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    summary = dict(iterations=args.iterations, cpu=cpu, cpu_workers=2,
                   delay_ms=args.sync_delay_ms, results=len(results),
                   failures=sum(r[3] is not None for r in results),
                   step5_timeouts=sum(bool(r[3] and 'step 5: TimeoutError' in r[3]) for r in results),
                   skipped=sum(r[4] is not None for r in results),
                   elapsed_s=time.monotonic() - started,
                   child_cpu_s=after.ru_utime + after.ru_stime - usage.ru_utime - usage.ru_stime)
    (args.output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps(summary), flush=True)
    if args.expect_timeouts:
        return int(summary['step5_timeouts'] != len(results) or summary['skipped'] != 0)
    return int(summary['failures'] != 0 or summary['skipped'] != 0)


if __name__ == '__main__':
    if sys.argv[1:] == ['--cpu-worker']:
        while True:
            pass
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--iterations', type=int, default=50)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--binary-dir', type=Path)
    parser.add_argument('--sync-delay-ms', type=int, default=75)
    parser.add_argument('--expect-timeouts', action='store_true')
    args = parser.parse_args()
    if args.iterations < 1 or not 0 <= args.sync_delay_ms <= 1000:
        parser.error('positive iterations and sync delay 0..1000 required')
    raise SystemExit(asyncio.run(run(args)))
