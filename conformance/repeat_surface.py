#!/usr/bin/env python3
"""Repeat real surface wire cases; optionally reproduce slow startup durability.

--without-readiness is a counterfactual: same binary/cases/perturbation, old
exec-only connection semantics. It must exhibit a step-0 timeout. This is a
controlled reproduction, not proof of the historical runner's disk latency.
"""
import argparse
import asyncio
import json
from pathlib import Path
import shutil

from harness import SUITES, cases_for, registry, report, run_case


async def run(args):
    impl = next(i for i in registry(SUITES / 'impls.toml') if i['name'] == args.implementation)
    if args.without_readiness:
        impl.pop('ready_line', None)
        # The clock connection is another readiness wait; the exec-only
        # counterfactual must disable it too, rather than mask startup stalls.
        impl.pop('fixture_clock', None)
    args.output.mkdir(parents=True, exist_ok=True)
    if args.slow_startup:
        if not shutil.which('strace'):
            raise RuntimeError('slow-startup proof requires strace')
        # The first fdatasync is the real Store Init append, before hello. Delay
        # only that call: action-path persistence/deadlines remain untouched.
        impl['command'] = ['strace', '-qq', '-e', 'trace=fdatasync',
                           '-e', 'inject=fdatasync:delay_enter=250ms:when=1',
                           '-o', str(args.output.resolve() / 'last-startup.trace'),
                           *impl['command']]
    cases = cases_for(SUITES, 'surface')
    results = []
    for iteration in range(args.iterations):
        batch = [await run_case(impl, 'surface', case) for case in cases]
        results.extend(batch)
        report(args.output / f'iteration-{iteration+1:03}.xml', batch)
        print(json.dumps(dict(iteration=iteration+1, failures=[
            dict(case=r[1], error=r[3]) for r in batch if r[3]])), flush=True)
    summary = dict(implementation=args.implementation, iterations=args.iterations,
                   slow_startup=args.slow_startup, without_readiness=args.without_readiness,
                   results=len(results), skipped=sum(r[4] is not None for r in results),
                   failures=sum(r[3] is not None for r in results),
                   step0_timeouts=sum(bool(r[3] and 'step 0: TimeoutError' in r[3]) for r in results))
    (args.output / 'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    print(json.dumps(summary), flush=True)
    if args.without_readiness:
        return int(not (summary['step0_timeouts'] > 0
                        and summary['failures'] == summary['step0_timeouts']))
    return int(bool(summary['failures']))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--iterations', type=int, default=50)
    parser.add_argument('--implementation', default='surface-rust-reference-in-process')
    parser.add_argument('--slow-startup', action='store_true')
    parser.add_argument('--without-readiness', action='store_true')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.iterations < 50:
        parser.error('repetition acceptance requires at least 50 iterations')
    raise SystemExit(asyncio.run(run(args)))
