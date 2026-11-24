#!/usr/bin/env python3
"""Check actual normal-dependency graphs, not a platform link qualification."""
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[3]


def dependencies(target, embedded):
    command = [
        "cargo", "tree", "--locked", "--offline", "-p", "coglet-runtime",
        "--target", target, "--edges", "normal", "--prefix", "none",
        "--format", "{p}", "--features", "native-engine",
    ]
    if embedded:
        command.append("--no-default-features")
    output = subprocess.check_output(command, cwd=ROOT, text=True)
    return {line.split()[0] for line in output.splitlines() if line.strip()}


for target in ("aarch64-apple-ios", "aarch64-apple-ios-sim", "aarch64-apple-darwin", "x86_64-apple-darwin"):
    embedded = dependencies(target, True)
    forbidden = embedded & {"coglet-proto", "iroh", "n0-dns-resolver", "system-configuration", "netdev"}
    if forbidden:
        raise SystemExit(f"{target}: embedded transport leakage: {sorted(forbidden)}")
    daemon = dependencies(target, False)
    if not {"coglet-proto", "iroh"} <= daemon:
        raise SystemExit(f"{target}: default device transport unexpectedly removed")
    print(f"{target}: embedded graph isolated; default device graph retains iroh/proto")
