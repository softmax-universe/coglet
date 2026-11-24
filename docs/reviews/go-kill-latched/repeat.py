import asyncio
import json
import multiprocessing
import os
from pathlib import Path
import sys
import time
import tomllib

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'conformance'))
from harness import run_case


def burn(stop):
    while not stop.is_set():
        sum(i*i for i in range(10000))


async def main():
    cpus = sorted(os.sched_getaffinity(0))
    # Own contention on one allowed CPU; do not saturate the shared pool host.
    os.sched_setaffinity(0, {cpus[0]})
    stop = multiprocessing.Event()
    burners = [multiprocessing.Process(target=burn, args=(stop,)) for _ in range(2)]
    for p in burners:
        p.start()
    impl = next(i for i in tomllib.loads((ROOT/'conformance/impls.toml').read_text())['implementations'] if i['name']=='surface-go-reference')
    impl['command'] = [str(Path(sys.argv[1]).resolve())]
    case = json.loads((ROOT/'conformance/surface/kill-latched.json').read_text())
    print(json.dumps(dict(cpu=cpus[0], burners=[p.pid for p in burners], fixture_clock=impl['fixture_clock'], started=time.time())), flush=True)
    try:
        results = []
        for n in range(1, 21):
            result = await run_case(impl, 'surface', case)
            results.append(result)
            print(json.dumps(dict(iteration=n, result=result)), flush=True)
            if result[3] or result[4]:
                raise RuntimeError(result)
        assert len(results)==20
        print('PASS 20/20 consecutive kill-latched under two CPU burners pinned to the same single CPU', flush=True)
    finally:
        stop.set()
        for p in burners:
            p.join(5)
            if p.is_alive():
                p.terminate()
                p.join()
        print(json.dumps(dict(burner_exitcodes=[p.exitcode for p in burners], ended=time.time())), flush=True)


if __name__=='__main__':
    asyncio.run(main())
