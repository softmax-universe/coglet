#!/usr/bin/env python3
"""Both native transports over ALL current runtime-side surface cases.

This tests negotiation only: current corpus has two runtime-under-test cases.
It is not operational/catalog/grant/durable-dispatch or native-I/O qualification.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET

root = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / "conformance"))
from wire import encode
subprocess.run(["cargo", "build", "--locked", "-p", "coglet-runtime", "--lib", "--bin", "cogletd"], cwd=root, check=True)
with tempfile.TemporaryDirectory(prefix="runtime-surface-", dir=os.environ.get("TMPDIR", root / "target")) as directory:
    work = Path(directory)
    cases = work / "cases/surface"
    cases.mkdir(parents=True)
    names = set()
    # Stage original files, unchanged, for the role implemented by this runtime.
    # New runtime-side case files automatically become required, not skipped.
    for path in (root / "conformance/surface").glob("*.json"):
        case = json.loads(path.read_text())
        if case["peer_role"] == "surface":
            names.add(case["name"])
            shutil.copyfile(path, cases / path.name)
    assert names, "no runtime-side cases"
    config = work / "binding.json"
    config.write_text(json.dumps({"surface": "surface:" + "a"*64, "permitted_coglets": ["coglet:" + "b"*64]}))
    hello = work / "hello.cbor"
    hello.write_bytes(encode({"type":"hello", "body":{"surface":"surface:"+"a"*64, "clock":0,"offer":{"versions":["0.1"]},"correlation_id":"1"*32}}))
    health = work / "health.cbor"
    health.write_bytes(encode({"type":"health","body":{"state":"stopped","clock":0,"pending":0,"correlation_id":"2"*32}}))
    libdir = root / "target/debug"
    native = work / "native-surface"
    subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-I", str(root / "crates/coglet-runtime/include"),
        str(root / "crates/coglet-runtime/tests/native_surface.c"), "-L", str(libdir), "-lcoglet_runtime", f"-Wl,-rpath,{libdir}", "-o", str(native)], check=True)
    subprocess.run([str(native), str(config), str(hello), str(health)], check=True)
    socket_dir = work / "private"
    socket_path = socket_dir / "s"
    server = subprocess.Popen([str(libdir / "cogletd"), "--surface-probe-socket", str(socket_path), str(config), str(os.geteuid())], cwd=root)
    try:
        deadline = time.monotonic() + 5
        while not socket_path.exists():
            if server.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError("surface socket did not become ready")
            time.sleep(.01)
        registry = work / "impls.toml"
        registry.write_text(f'''[[implementations]]
name = "runtime-socket"
suites = ["surface"]
peer_roles = ["surface"]
transport = "unix"
socket = {json.dumps(str(socket_path))}
[implementations.fixtures.runtime-binding]

[[implementations]]
name = "runtime-c-abi"
suites = ["surface"]
peer_roles = ["surface"]
transport = "stdio"
command = {json.dumps([sys.executable, str(root / "crates/coglet-runtime/tools/ffi-bridge.py"), str(libdir / "libcoglet_runtime.so"), str(config)])}
[implementations.fixtures.runtime-binding]
''')
        report = work / "results.xml"
        subprocess.run([sys.executable, str(root / "conformance/run"), "--registry", str(registry), "--cases", str(cases.parent), "--junit", str(report)], cwd=root, check=True)
        tree = ET.parse(report).getroot()
        actual = [case.attrib["name"] for case in tree.findall("testcase")]
        assert len(actual) == 2 * len(names), actual
        assert all(actual.count(name) == 2 for name in names), actual
        assert tree.attrib["failures"] == "0" and tree.attrib["skipped"] == "0", tree.attrib
        shutil.copyfile(report, root / "target/runtime-surface.xml")
        print(f"Runtime-side negotiation gate: {len(names)} original cases x 2 native transports, zero failures/skips. Operational qualification remains outstanding.")
    finally:
        server.terminate()
        server.wait(timeout=5)
