#!/usr/bin/env python3
"""Test-only pipe proxy and bounded post-wire oracle custody.

The runner owns the state directory and final cleanup. Hooks never spawn children.
The implementation child has private pipes (not the harness stdout/stderr), so
harness process-group cleanup cannot kill a still-running bounded native guest.
Teardown must observe its real oracle result and process exit or fail the case.
"""
import asyncio
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from receiver_oracles import validate


def identity(pid):
    try:
        # comm can contain spaces; field 22 follows the final ')'.
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        return fields[19], fields[0]
    except FileNotFoundError:
        return None


def paths():
    root = Path(os.environ["COGLET_FIXTURE_RECEIVER_STATE"])
    case = os.environ["COGLET_CONFORMANCE_CASE"]
    if not case or any(c not in "abcdefghijklmnopqrstuvwxyz-" for c in case):
        raise ValueError("invalid case")
    return root / (case + ".custody.json"), root / (case + ".result.json")


def stop_owned(path):
    if not path.exists():
        return
    saved = json.loads(path.read_text())
    current = identity(saved["pid"])
    if current and current[0] == saved["start"] and current[1] != "Z":
        os.killpg(saved["pid"], signal.SIGKILL)


async def bridge():
    custody, result = paths()
    if custody.exists() or result.exists():
        raise RuntimeError("stale case custody; use a fresh runner state directory")
    env = {**os.environ, "COGLET_RECEIVER_RESULT": str(result)}
    child = await asyncio.create_subprocess_exec(
        "target/release/examples/runtime_receiver_peer",
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        start_new_session=True, env=env,
    )
    start = identity(child.pid)
    if start is None:
        raise RuntimeError("child exited before custody")
    custody.write_text(json.dumps({"pid": child.pid, "start": start[0]}))
    writer, stdout, stderr = child.stdin, child.stdout, child.stderr
    assert writer is not None and stdout is not None and stderr is not None

    async def feed():
        loop = asyncio.get_running_loop()
        while data := await loop.run_in_executor(None, os.read, 0, 65536):
            writer.write(data)
            await writer.drain()
        writer.close()

    async def copy(source, fd):
        while data := await source.read(65536):
            view = memoryview(data)
            while view:
                view = view[os.write(fd, view):]

    tasks = [asyncio.create_task(feed()), asyncio.create_task(copy(stdout, 1)),
             asyncio.create_task(copy(stderr, 2))]
    code = await child.wait()
    await asyncio.gather(*tasks[1:])
    tasks[0].cancel()
    # os._exit avoids waiting for the blocking stdin executor after a final reply.
    os._exit(code if code >= 0 else 1)


def teardown():
    custody, result = paths()
    deadline = time.monotonic() + 9
    validated = False
    try:
        while time.monotonic() < deadline:
            if result.exists():
                report = json.loads(result.read_text())
                if report.get("ok") is not True:
                    raise RuntimeError(f"actual receiver oracle failed: {report}")
                if not validated:
                    validate(json.loads(Path(str(result) + ".oracle.json").read_text()))
                    validated = True
                saved = json.loads(custody.read_text())
                current = identity(saved["pid"])
                if current is None or current[0] != saved["start"] or current[1] == "Z":
                    return
            time.sleep(.02)
        raise RuntimeError("receiver guest/oracle did not finish within teardown bound")
    finally:
        stop_owned(custody)


if __name__ == "__main__":
    if sys.argv[1:] == ["teardown"]:
        teardown()
    elif sys.argv[1:] == ["bridge"]:
        asyncio.run(bridge())
    else:
        raise SystemExit("expected bridge or teardown")
