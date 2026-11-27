#!/usr/bin/env python3
"""Non-required only: twenty consecutive cartpole live/replay runs under CPU load.

Never retries a failure. Runtime deadlines/grants and the original pytest test
are unchanged. Affinity is inherited by pytest, daemon and replay proof process.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET

TEST = "sim/tests/test_native.py::test_cartpole_episode_live_and_replayed"
RUNS = 20


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    cpu = min(os.sched_getaffinity(0))
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=repo).strip():
        raise RuntimeError("qualification requires a clean committed source tree")
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()
    paths = {
        "COGLET_SANDBOX_DAEMON": repo / "target/release/cogletd",
        "COGLET_SANDBOX_PROOF": repo / "target/release/coglet-sandbox-proof",
        "COGLET_SANDBOX_WASM": repo / "target/sandbox/pilot.wasm",
    }
    hashes = {key: hashlib.sha256(path.read_bytes()).hexdigest() for key, path in paths.items()}
    env = dict(os.environ, TMPDIR=os.environ.get("TMPDIR", tempfile.gettempdir()),
               **{key: str(path) for key, path in paths.items()})
    summary = dict(head=head, test=TEST, required=False, requested_runs=RUNS,
                   cpu=cpu, hashes=hashes, runs=[], loads=[], all_passed=False)
    children = []
    try:
        for _ in range(2):
            children.append(subprocess.Popen(["taskset", "-c", str(cpu), sys.executable,
                                               "-c", "while True: pass"]))
        summary["load_pids"] = [child.pid for child in children]
        for index in range(1, RUNS + 1):
            if any(child.poll() is not None for child in children):
                raise RuntimeError("owned CPU-load child exited")
            run_dir = output / f"run-{index:02d}"
            run_dir.mkdir()
            junit = run_dir / "junit.xml"
            with tempfile.TemporaryDirectory(prefix="native-stability-") as temp:
                pytest_dir = Path(temp) / "pytest"
                command = ["taskset", "-c", str(cpu), "uv", "run", "--project", "sim",
                           "--locked", "pytest", TEST, "-v", "-s", "--basetemp",
                           str(pytest_dir), "--junitxml", str(junit)]
                started = time.monotonic()
                with (run_dir / "pytest.log").open("w") as log:
                    process = subprocess.Popen(command, cwd=repo, env=env, stdout=log,
                                               stderr=subprocess.STDOUT, start_new_session=True)
                    try:
                        code = process.wait(timeout=300)
                    except BaseException:
                        # Kill only our owned session, including daemon/proof descendants.
                        try:
                            os.killpg(process.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                        process.wait()
                        raise
                # Retain raw daemon diagnostics, not twenty large successful WALs.
                # The unchanged pytest test independently proves live/replay identity.
                for name in ("daemon.log", "native-failure.json"):
                    for source in pytest_dir.rglob(name):
                        shutil.copy2(source, run_dir / name)
                tests = list(ET.parse(junit).getroot().iter("testcase")) if junit.exists() else []
                passed = (code == 0 and len(tests) == 1 and
                          tests[0].get("name") == TEST.split("::")[1] and
                          not any(tests[0].find(tag) is not None
                                  for tag in ("skipped", "failure", "error")))
                summary["runs"].append(dict(index=index, command=command, returncode=code,
                                            elapsed=time.monotonic() - started,
                                            tests=len(tests), passed=passed))
                (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
                print(f"cartpole CPU-load qualification {index}/{RUNS}: "
                      f"{'PASS' if passed else 'FAIL'}", flush=True)
                if not passed:
                    return 1
        if any(child.poll() is not None for child in children):
            raise RuntimeError("owned CPU-load child exited during final pass")
        summary["all_passed"] = True
        return 0
    except BaseException as error:
        summary["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        for child in children:
            if child.returncode is None:
                child.kill()
                _, status, usage = os.wait4(child.pid, 0)
                child.returncode = os.waitstatus_to_exitcode(status)
                summary["loads"].append(dict(pid=child.pid, returncode=child.returncode,
                                              user_seconds=usage.ru_utime,
                                              system_seconds=usage.ru_stime, reaped=True))
            else:
                summary["loads"].append(dict(pid=child.pid, returncode=child.returncode,
                                              user_seconds=None, system_seconds=None, reaped=True))
        usage = resource.getrusage(resource.RUSAGE_CHILDREN)
        summary["children_cpu_seconds"] = usage.ru_utime + usage.ru_stime
        (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")


if __name__ == "__main__":
    raise SystemExit(main())
