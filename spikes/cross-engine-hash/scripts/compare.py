#!/usr/bin/env python3
"""Compare per-step hash logs from several engine configurations.

usage: compare.py [--expect equal|differ] [--label NAME] cfgA=fileA cfgB=fileB ...

The first config is the reference. For each other config, prints the number of steps whose guest
hash AND host-side state hash match, the first diverging step, and (when the logs carry the state
blob, i.e. XHASH_STATE=1) which state lanes differ and their bit patterns.
Exit status: 0 if the outcome matches --expect (default equal), 1 otherwise.
"""
import struct
import sys


def load(path):
    rows = {}
    for line in open(path):
        p = line.split()
        if len(p) >= 3:
            rows[int(p[0])] = (p[1], p[2], bytes.fromhex(p[3]) if len(p) > 3 else None)
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
    a = sys.argv[1:]
    expect = "equal"
    label = ""
    while a and a[0].startswith("--"):
        if a[0] == "--expect":
            expect = a[1]
        elif a[0] == "--label":
            label = a[1]
        a = a[2:]
    cfgs = [x.split("=", 1) for x in a]
    data = [(n, load(p)) for n, p in cfgs]
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
    sys.exit(0 if got == expect else 1)


main()
