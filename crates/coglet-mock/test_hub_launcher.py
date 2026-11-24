#!/usr/bin/env python3
"""Exercise the private bootstrap/EOF contract used by native client tests."""
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def main():
    subprocess.run(
        ["cargo", "build", "--locked", "-j", "2", "-p", "coglet-mock", "--example", "hub"],
        cwd=ROOT, check=True,
    )
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target.is_absolute():
        target = ROOT / target
    # Public fixture Ed25519 key, never a private client seed.
    device = "d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737"
    with subprocess.Popen(
        [str(target / "debug/examples/hub"), device], cwd=ROOT,
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True,
    ) as process:
        try:
            output, errors = process.communicate(input="", timeout=30)
            assert process.returncode == 0, errors
            lines = output.splitlines()
            assert len(lines) == 1, "expected one private bootstrap line"
            bootstrap = json.loads(lines[0])
            assert bootstrap["device"] == device
            assert bootstrap["alpn"] == "coglet/hub/0"
            assert len(bootstrap["hub"]) == 64
            assert len(bytes.fromhex(bootstrap["invite"])) == 32
            assert bootstrap["addresses"]
            assert all(a.startswith("127.0.0.1:") or a.startswith("[::1]:") for a in bootstrap["addresses"])
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
    print("hub_launcher_bootstrap_and_eof_shutdown: passed")


if __name__ == "__main__":
    main()
