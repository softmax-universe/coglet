#!/usr/bin/env python3
"""Real installed-config C pointer smoke, compiled Wasm and checksum-checked WAL.

Build coglet-runtime with native-engine (and ffi-test-hooks for --panic).
Install a fresh fixture with install_speech_echo.py first; use a new WAL per run.
"""
import argparse
import ctypes as C
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "conformance"))
from wire import encode, decode


def read_records(path):
    raw = Path(path).read_bytes()
    records, previous, offset = [], None, 0
    while offset < len(raw):
        size = int.from_bytes(raw[offset:offset + 4], "big")
        offset += 4
        frame = raw[offset:offset + size]
        assert len(frame) == size and size > 0
        offset += size
        tx = decode(frame)
        checksum = tx.pop("hash")
        assert tx["previous"] == previous
        assert hashlib.sha256(encode(tx)).hexdigest() == checksum
        previous = checksum
        if tx["journal"] is not None:
            records.append(tx["journal"])
    return records


def smoke(library, installed, trap=False, panic=False):
    lib = C.CDLL(str(Path(library).resolve()))
    lib.coglet_runtime_configure.argtypes = [C.c_char_p, C.c_size_t]
    lib.coglet_runtime_start.argtypes = [C.c_char_p, C.c_size_t]
    lib.coglet_surface_open.argtypes = [C.c_uint32, C.POINTER(C.c_void_p)]
    lib.coglet_surface_send.argtypes = [C.c_void_p, C.c_char_p, C.c_size_t]
    lib.coglet_surface_recv.argtypes = [C.c_void_p, C.c_void_p, C.c_size_t, C.POINTER(C.c_size_t)]
    lib.coglet_surface_close.argtypes = [C.c_void_p]
    lib.coglet_surface_close.restype = None
    root = Path(installed)
    binding = json.loads((root / "binding.json").read_text())
    config = (root / "runtime.json").read_bytes()
    runtime = json.loads(config)
    assert not Path(runtime["wal"]).exists(), "use fresh install/WAL per smoke"
    raw = json.dumps(binding).encode()
    assert lib.coglet_runtime_configure(raw, len(raw)) == 0
    assert lib.coglet_runtime_start(config, len(config)) == 0, "real engine admission failed"
    link = C.c_void_p()
    assert lib.coglet_surface_open(1, C.byref(link)) == 0

    def send(kind, body):
        raw = encode({"type": kind, "body": body})
        status = lib.coglet_surface_send(link, raw, len(raw))
        assert status == 0, (kind, status)

    def recv():
        n = C.c_size_t()
        status = lib.coglet_surface_recv(link, None, 0, C.byref(n))
        if status == 1:
            return None
        assert status == 2, ("recv", status)
        buf = C.create_string_buffer(n.value)
        assert lib.coglet_surface_recv(link, buf, n.value, C.byref(n)) == 0
        return decode(buf.raw[:n.value])

    send("hello", {"surface": binding["surface"], "offer": {"versions": ["0.1"]}, "clock": 1000, "correlation_id": "1" * 32})
    assert recv()["type"] == "hello_reply"
    operations = ["speech.start", "speech.end", "speech.speak_chunk"]
    schema = "https://coglet.dev/spec/messages/speech.schema.json#/$defs/"
    send("capabilities", {"revision": 1, "clock": 1100, "entries": [{"name": "speech.control",
        "events": operations[:2], "reads": [], "actions": operations[2:],
        "exclusive": ["audio.mic", "audio.speaker"], "schemas": dict(zip(operations, [schema + x for x in ["speech-start", "speech-end", "speak-chunk"]]))}], "correlation_id": "2" * 32})
    send("grants", {"epoch": 1, "entries": [{"coglet": binding["permitted_coglets"][0], "operations": operations,
        "resources": ["audio.mic", "audio.speaker"], "expires": 100_000_000}], "correlation_id": "3" * 32})
    send("event", {"source": binding["surface"], "kind": "speech.start", "seq": 1, "logical_time": 1200,
        "payload": {"utterance": "a" * 32, "position_ms": 0}, "correlation_id": "4" * 32})
    text = "Actual compiled speech echo — 世界"
    send("event", {"source": binding["surface"], "kind": "speech.end", "seq": 2, "logical_time": 1300,
        "payload": {"utterance": "a" * 32, "revision": 1, "text": text, "position_ms": 10}, "correlation_id": "4" * 32})
    if not trap:
        message = recv()
        while message and message["type"] == "health":
            message = recv()
        assert message and message["type"] == "act", message
        action = message["body"]["action"]
        assert action["capability"] == "speech.speak_chunk" and action["args"]["text"] == text
        assert action["args"]["final"] and action["args"]["chunk"] == 0
    if trap:
        message = recv()
        while message and message["type"] == "health":
            message = recv()
        assert message is None, message
    assert lib.coglet_runtime_status() == (-2 if trap else 0)
    if panic:
        assert lib.coglet_runtime_test_panic() == -2
        assert lib.coglet_runtime_status() == -2
        n = C.c_size_t(999)
        assert lib.coglet_surface_recv(link, None, 0, C.byref(n)) == -2
        assert n.value == 0
        assert lib.coglet_runtime_stop() == 1, "live service link must prevent dropping runtime"
        assert lib.coglet_runtime_start(config, len(config)) == -2
        refused = C.c_void_p()
        assert lib.coglet_surface_open(1, C.byref(refused)) == -2 and not refused.value
    lib.coglet_surface_close(link)
    assert lib.coglet_runtime_stop() == 0
    records = read_records(runtime["wal"])
    steps = [r for r in records if "status" in r]
    assert len(steps) == 1, [(r.get("status"), r.get("events")) for r in records]
    step = steps[0]
    assert step["status"] == ("aborted" if trap else "committed"), step.get("failure")
    assert step["usage"]["compute"] > 0
    assert len(step["actions"]) == (0 if trap else 1)
    if trap:
        assert step["input_hash"] == step["output_hash"]
    # Recovery exercises real WAL reopen and native-engine admission. No grants
    # or new input: a committed/failed batch must not run a second time.
    assert lib.coglet_runtime_start(config, len(config)) == 0
    assert lib.coglet_runtime_stop() == 0
    assert len(read_records(runtime["wal"])) == len(records)
    print(json.dumps({"compiled_wasm": True, "status": step["status"], "actions": len(step["actions"]),
                      "ffi_panic_caught_and_restarted": panic, "wal_checksums_verified": True}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--library", required=True)
    parser.add_argument("--installed", required=True)
    parser.add_argument("--expect-trap", action="store_true")
    parser.add_argument("--panic", action="store_true")
    args = parser.parse_args()
    smoke(args.library, args.installed, args.expect_trap, args.panic)
