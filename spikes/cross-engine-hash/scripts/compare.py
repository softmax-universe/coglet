#!/usr/bin/env python3
"""Compare per-step hash logs from several engine configurations.

usage: compare.py [--expect equal|differ] [--label NAME] cfgA=fileA cfgB=fileB ...

The first config is the reference. For each other config, prints the number of steps whose guest
hash AND host-side state hash match, the first diverging step, and (when the logs carry the state
blob, i.e. XHASH_STATE=1) which state lanes differ and their bit patterns.
Exit status: 0 if the outcome matches --expect (default equal), 1 otherwise.
"""
import argparse
import re
import struct
import sys


STEPS = 1000


def load(path):
    """Reject incomplete/invalid evidence before testing equality OR divergence."""
    rows = {}
    with open(path) as stream:
        for line_no, line in enumerate(stream, 1):
            p = line.split()
            if (len(p) not in (3, 4) or not re.fullmatch(r"0|[1-9][0-9]*", p[0])
                    or any(not re.fullmatch(r"[0-9a-f]{16}", h) for h in p[1:3])):
                raise ValueError(f"{path}:{line_no}: malformed hash row")
            step = int(p[0])
            if step != len(rows):
                raise ValueError(f"{path}:{line_no}: expected step {len(rows)}, got {step}")
            if p[1] != p[2]:
                raise ValueError(f"{path}:{line_no}: guest/host hash mismatch")
            blob = None
            if len(p) == 4:
                if not re.fullmatch(r"[0-9a-f]{832}", p[3]):
                    raise ValueError(f"{path}:{line_no}: malformed 416-byte state blob")
                blob = bytes.fromhex(p[3])
            rows[step] = (p[1], p[2], blob)
    if len(rows) != STEPS:
        raise ValueError(f"{path}: expected {STEPS} steps, got {len(rows)}")
    return rows


def lanes(blob):
    out = []
    for i in range(32):
        out.append((f"x[{i}]", struct.unpack_from("<I", blob, i * 4)[0], 8))
    for i in range(32):
        out.append((f"y[{i}]", struct.unpack_from("<Q", blob, 128 + i * 8)[0], 16))
    for i in range(4):
        out.append((f"ctr[{i}]", struct.unpack_from("<Q", blob, 384 + i * 8)[0], 16))
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--expect", choices=("equal", "differ"), default="equal")
    parser.add_argument("--label", default="")
    parser.add_argument("configs", nargs="+")
    args = parser.parse_args()
    expect, label = args.expect, args.label
    if len(args.configs) < 2 or any("=" not in c for c in args.configs):
        parser.error("provide at least two NAME=FILE configurations")
    cfgs = [x.split("=", 1) for x in args.configs]
    try:
        data = [(n, load(p)) for n, p in cfgs]
    except (OSError, ValueError) as exc:
        print(f"INVALID EVIDENCE: {exc}", file=sys.stderr)
        return 1
    ref_name, ref = data[0]
    all_equal = True
    print(f"### {label}" if label else "###")
    print(f"reference: {ref_name} ({len(ref)} steps)")
    for name, rows in data[1:]:
        if len(rows) != len(ref):
            print(f"  {name}: only {len(rows)} steps produced")
        same = [s for s in sorted(ref) if s in rows and rows[s][:2] == ref[s][:2]]
        diff = [s for s in sorted(ref) if s not in rows or rows[s][:2] != ref[s][:2]]
        ok = not diff and len(rows) == len(ref)
        all_equal &= ok
        print(f"  {name}: {len(same)}/{len(ref)} steps equal" + ("" if ok else f", first divergence at step {diff[0]}"))
        if diff and rows.get(diff[0]) and ref[diff[0]][2] and rows[diff[0]][2]:
            la, lb = lanes(ref[diff[0]][2]), lanes(rows[diff[0]][2])
            for (n, x, w), (_, y, _) in zip(la, lb):
                if x != y:
                    print(f"      {n}: {ref_name}=0x{x:0{w}x} {name}=0x{y:0{w}x}")
    got = "equal" if all_equal else "differ"
    verdict = "PASS" if got == expect else "FAIL"
    print(f"=> {got} (expected {expect}): {verdict}")
    return 0 if got == expect else 1


if __name__ == "__main__":
    sys.exit(main())
