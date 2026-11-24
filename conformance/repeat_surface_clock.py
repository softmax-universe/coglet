#!/usr/bin/env python3
"""Run full surface loops on four real adapter variants under owned CPU load.

Optionally delay every post-initialization sync (including negotiation), and
compare explicit fixture-clock binding with the unchanged wall-clock binding.
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


async def run(args):
    if not hasattr(os, 'sched_setaffinity') or not shutil.which('strace'):
        raise RuntimeError('Linux affinity and strace required')
    args.output.mkdir(parents=True, exist_ok=True)
    cpu = min(os.sched_getaffinity(0))
    os.sched_setaffinity(0, {cpu})
    workers = [subprocess.Popen([sys.executable, __file__, '--cpu-worker']) for _ in range(2)]
    started = time.monotonic()
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    results = []
    try:
        implementations = [copy.deepcopy(i) for i in registry(SUITES / 'impls.toml')
                           if i['name'] in ('surface-rust-reference', 'mock-surface',
                                            'surface-rust-reference-in-process', 'mock-surface-in-process')]
        assert len(implementations) == 4
        cases = cases_for(SUITES, 'surface')
        if args.case:
            cases = [c for c in cases if c['name'] in args.case]
            assert {c['name'] for c in cases} == set(args.case)
        for iteration in range(1, args.iterations + 1):
            batch = []
            for impl in implementations:
                for case in cases:
                    traced = copy.deepcopy(impl)
                    if args.without_fixture_clock:
                        traced.pop('fixture_clock', None)
                    if args.sync_delay_ms:
                        trace = args.output.resolve() / f'{iteration:03}-{impl["name"]}-{case["name"]}.trace'
                        traced['command'] = ['strace', '-f', '-qq', '-tt', '-T', '-e', 'trace=fdatasync,write',
                                             '-e', f'inject=fdatasync:delay_enter={args.sync_delay_ms}ms:when=2+',
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
                   sync_delay_ms=args.sync_delay_ms, fixture_clock=not args.without_fixture_clock,
                   results=len(results), failures=sum(r[3] is not None for r in results),
                   timeouts=sum(bool(r[3] and 'TimeoutError' in r[3]) for r in results),
                   skipped=sum(r[4] is not None for r in results),
                   elapsed_s=time.monotonic()-started,
                   child_cpu_s=after.ru_utime+after.ru_stime-usage.ru_utime-usage.ru_stime)
    (args.output / 'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    print(json.dumps(summary), flush=True)
    if args.expect_timeouts:
        return int(summary['timeouts'] == 0)
    return int(summary['failures'] != 0)


if __name__ == '__main__':
    if sys.argv[1:] == ['--cpu-worker']:
        while True:
            pass
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--iterations', type=int, default=50)
    parser.add_argument('--sync-delay-ms', type=int, default=0)
    parser.add_argument('--without-fixture-clock', action='store_true')
    parser.add_argument('--expect-timeouts', action='store_true')
    parser.add_argument('--case', action='append')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.iterations < 1 or not 0 <= args.sync_delay_ms <= 1000:
        parser.error('iterations positive, sync delay 0..1000')
    raise SystemExit(asyncio.run(run(args)))
