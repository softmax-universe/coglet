#!/usr/bin/env python3
"""Non-required ADR-0027 restart stability proof, with raw execution evidence."""
import argparse
import datetime
import hashlib
import json
import multiprocessing
import os
from pathlib import Path
import subprocess
import time


def load(stop, measurements):
    started = time.process_time()
    blocks = 0
    data = bytes(65536)
    while not stop.is_set():
        data = hashlib.sha256(data).digest() * 2048
        blocks += 1
    measurements.put({"pid": os.getpid(), "cpu_seconds": time.process_time() - started, "blocks": blocks})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--count", type=int, default=20)
    parser.add_argument("--load-workers", type=int, default=2)
    args = parser.parse_args()
    if args.count < 1 or args.load_workers < 1:
        parser.error("positive count and load-workers required")
    args.output.mkdir(parents=True, exist_ok=False)
    repo = Path(__file__).resolve().parents[3]
    sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=repo, text=True).strip():
        raise SystemExit("Commit qualification inputs before collecting exact-head evidence")
    stop = multiprocessing.Event()
    measurements = multiprocessing.Queue()
    workers = [multiprocessing.Process(target=load, args=(stop, measurements)) for _ in range(args.load_workers)]
    for worker in workers:
        worker.start()
    print(json.dumps({"head": sha, "load_pids": [w.pid for w in workers]}), flush=True)
    results = []
    started = time.monotonic()
    try:
        for iteration in range(1, args.count + 1):
            command = ["cargo", "test", "-p", "coglet-runtime", "--features", "native-engine", "--locked", "--test", "device_lifecycle", "running_coglet_resumes_execution_after_daemon_restart", "--", "--exact", "--nocapture"]
            at = datetime.datetime.now(datetime.timezone.utc).isoformat()
            before = time.monotonic()
            with (args.output / f"run-{iteration:02}.log").open("w") as log:
                completed = subprocess.run(command, cwd=repo, stdout=log, stderr=subprocess.STDOUT, timeout=180)
            raw = (args.output / f"run-{iteration:02}.log").read_text()
            passed = completed.returncode == 0 and "1 passed; 0 failed; 0 ignored" in raw
            row = {"iteration": iteration, "at": at, "seconds": time.monotonic() - before, "returncode": completed.returncode, "passed": passed, "load_alive": all(w.is_alive() for w in workers)}
            results.append(row)
            print(json.dumps(row), flush=True)
            if not passed or not row["load_alive"]:
                break
    finally:
        stop.set()
        for worker in workers:
            worker.join(timeout=10)
        loads = [measurements.get(timeout=5) for _ in workers]
        summary = {"head": sha, "requested": args.count, "runs": results, "load": loads, "elapsed_seconds": time.monotonic() - started}
        summary["passed"] = len(results) == args.count and all(r["passed"] and r["load_alive"] for r in results) and all(r["cpu_seconds"] > 0 for r in loads)
        (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
        print(json.dumps(summary), flush=True)
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
