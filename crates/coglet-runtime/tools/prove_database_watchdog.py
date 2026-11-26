#!/usr/bin/env python3
"""ADR-0041 real WAMR/SQLite/WAL delay proof, never a response oracle.

Requires an already-built database_adapter; never invokes Cargo. Evidence goes
in a new external directory. Only the driver reads the frozen case/expectation;
the forwarding subprocess sees opaque request frames and actual adapter output.
The injected stall demonstrates the runner boundary, not a historical CI cause.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import struct
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[3]
MAX_FRAME = 4 * 1024 * 1024


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def exact(stream, size):
    chunks = bytearray()
    while len(chunks) < size:
        chunk = stream.read(size - len(chunks))
        if not chunk:
            raise EOFError("truncated frame")
        chunks.extend(chunk)
    return bytes(chunks)


def frame(stream):
    header = stream.read(4)
    if not header:
        return None
    if len(header) != 4:
        raise EOFError("truncated header")
    size = struct.unpack(">I", header)[0]
    if not 0 < size <= MAX_FRAME:
        raise ValueError("invalid frame size")
    return header + exact(stream, size)


def forward(args):
    # No corpus, case name, expected reply, or decoded request enters this path.
    command = [sys.executable, str(ROOT / "conformance/engine/adapter.py"),
               str(args.adapter), "real"]
    with args.events.open("x", buffering=1) as events:
        def event(kind, **fields):
            events.write(json.dumps(dict(kind=kind, monotonic=time.monotonic(), **fields)) + "\n")
        child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        assert child.stdin is not None and child.stdout is not None
        event("started", command=command, pid=child.pid)
        try:
            while (request := frame(sys.stdin.buffer)) is not None:
                event("received", sha256=sha(request), size=len(request))
                start = time.monotonic()
                time.sleep(args.delay_ms / 1000)
                event("delayed", elapsed_ms=(time.monotonic() - start) * 1000)
                child.stdin.write(request)
                child.stdin.flush()
                event("forwarded", sha256=sha(request), size=len(request))
                reply = frame(child.stdout)
                if reply is None:
                    raise EOFError("actual adapter closed without response")
                event("response", sha256=sha(reply), size=len(reply))
                sys.stdout.buffer.write(reply)
                sys.stdout.buffer.flush()
        finally:
            child.stdin.close()
            child.terminate()
            try:
                child.wait(timeout=2)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()


def run(command, evidence, label):
    (evidence / (label + ".command.json")).write_text(json.dumps(command, indent=2) + "\n")
    with (evidence / (label + ".log")).open("x") as log:
        result = subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, timeout=30)
    (evidence / (label + ".exit")).write_text(str(result.returncode) + "\n")
    return result.returncode


def proof(args):
    evidence = args.evidence.resolve()
    if evidence.is_relative_to(ROOT):
        raise ValueError("evidence must be outside the repository")
    evidence.mkdir(parents=True, exist_ok=False)
    adapter = args.adapter.resolve(strict=True)
    if not os.access(adapter, os.X_OK):
        raise ValueError("adapter must already be executable; build separately")
    corpus = ROOT / "conformance/database/cases.json"
    before = corpus.read_bytes()
    cases = json.loads(before)
    selected = [case for case in cases if case["name"] == "db-paging-cursor"]
    assert len(selected) == 1
    case = selected[0]
    assert [s["within_ms"] for s in case["steps"] if "expect" in s] == [500]
    subset = evidence / "cases/database"
    subset.mkdir(parents=True)
    # Copy the entire selected object without changing its fields or time bounds.
    (subset / "cases.json").write_text(json.dumps(selected, indent=2) + "\n")
    sys.path.insert(0, str(ROOT / "conformance"))
    from wire import encode
    requests = [encode(s["send"]) for s in case["steps"] if "send" in s]
    assert len(requests) == 1
    raw = struct.pack(">I", len(requests[0])) + requests[0]
    (evidence / "request.frame").write_bytes(raw)
    metadata = dict(head=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                    adapter=str(adapter), adapter_sha256=sha(adapter.read_bytes()),
                    corpus_sha256=sha(before), request_frame_sha256=sha(raw),
                    driver_sha256=sha(Path(__file__).read_bytes()), delay_ms=args.delay_ms)
    (evidence / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    for source, name in (
        (Path(__file__), "driver.source.py"),
        (ROOT / "crates/coglet-runtime/examples/database_adapter.rs", "adapter.source.rs"),
        (ROOT / "conformance/harness.py", "harness.source.py"),
        (ROOT / "conformance/engine/adapter.py", "bridge.source.py"),
    ):
        (evidence / name).write_bytes(source.read_bytes())
    results = {}
    for label in ("red", "green"):
        wrapper = [sys.executable, str(Path(__file__).resolve()), "--forward",
                   "--adapter", str(adapter), "--events", str(evidence / (label + ".events.jsonl")),
                   "--delay-ms", str(args.delay_ms)]
        common = [sys.executable, str(ROOT / "conformance/run"), "--cases", str(evidence / "cases"),
                  "--junit", str(evidence / (label + ".xml"))]
        if label == "red":
            command = common + ["--impl", shlex.join(wrapper), "--suite", "database", "--peer-role", "guest"]
        else:
            registry = evidence / "impls.toml"
            registry.write_text('[[implementations]]\nname = "actual-delayed-database"\nsuites = ["database"]\n'
                                'peer_roles = ["guest"]\ntransport = "stdio"\n'
                                'database_functional_watchdog = true\ncommand = ' + json.dumps(wrapper) + '\n')
            command = common + ["--registry", str(registry)]
        code = run(command, evidence, label)
        report = ET.parse(evidence / (label + ".xml")).getroot()
        tests = report.findall("testcase")
        assert len(tests) == 1 and tests[0].attrib["name"] == "db-paging-cursor", report.attrib
        assert not tests[0].findall("skipped"), report.attrib
        events = [json.loads(line) for line in (evidence / (label + ".events.jsonl")).read_text().splitlines()]
        received = [e for e in events if e["kind"] == "received"]
        assert len(received) == 1 and received[0]["sha256"] == sha(raw), events
        failure = tests[0].find("failure")
        if label == "red":
            assert code == 1 and failure is not None and "step 1: TimeoutError" in (failure.text or ""), report.attrib
        else:
            assert code == 0 and failure is None and report.attrib["failures"] == "0", report.attrib
            delays = [e["elapsed_ms"] for e in events if e["kind"] == "delayed"]
            assert len(delays) == 1 and 500 < delays[0] < 5000, delays
            forwarded = [e for e in events if e["kind"] == "forwarded"]
            assert len(forwarded) == 1 and forwarded[0]["sha256"] == sha(raw), events
            assert len([e for e in events if e["kind"] == "response"]) == 1, events
        results[label] = dict(exit=code, junit=report.attrib, events=events)
    assert corpus.read_bytes() == before, "frozen corpus changed during proof"
    assert sha(adapter.read_bytes()) == metadata["adapter_sha256"], "adapter changed during proof"
    (evidence / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    print(f"PASS: original 500ms direct RED; opted-in 5000ms real WAMR/SQLite/WAL GREEN; evidence={evidence}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--adapter", type=Path, default=ROOT / "target/debug/examples/database_adapter")
    parser.add_argument("--evidence", type=Path)
    parser.add_argument("--delay-ms", type=int, default=1200)
    parser.add_argument("--forward", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--events", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not 500 < args.delay_ms < 5000:
        parser.error("delay must be strictly between 500 and 5000ms")
    if args.forward:
        if args.events is None:
            parser.error("forwarder requires --events")
        forward(args)
    else:
        if args.evidence is None:
            parser.error("proof requires a new --evidence directory outside the repository")
        proof(args)


if __name__ == "__main__":
    main()
