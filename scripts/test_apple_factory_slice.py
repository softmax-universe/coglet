#!/usr/bin/env python3
"""Partition the actual native Factory test manifest; never accept an empty slice."""
import argparse
import subprocess

CARGO = ["cargo", "test", "--locked", "-p", "coglet-runtime", "--features",
         "native-engine", "--test", "factory_database", "--"]
COUNTS = {"runtime": 8, "goals": 6, "research": 4, "backlog": 1}


def partition(names):
    groups = {key: [] for key in COUNTS}
    for name in names:
        if name.startswith("factory_goals::"):
            key = "goals"
        elif name.startswith("factory_research::backlog::"):
            key = "backlog"
        elif name.startswith("factory_research::"):
            key = "research"
        else:
            key = "runtime"
        groups[key].append(name)
    if len(set(names)) != len(names):
        raise ValueError("duplicate test identifiers")
    for key, expected in COUNTS.items():
        if len(groups[key]) != expected:
            raise ValueError(f"{key}: expected {expected} tests, got {len(groups[key])}")
    return groups


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("slice", choices=COUNTS)
    parser.add_argument("--list-only", action="store_true")
    args = parser.parse_args()
    listed = subprocess.run(CARGO + ["--list"], check=True, text=True,
                            stdout=subprocess.PIPE).stdout
    names = [line.removesuffix(": test") for line in listed.splitlines()
             if line.endswith(": test")]
    groups = partition(names)
    print("Factory manifest: " + ", ".join(f"{k}={len(v)}" for k, v in groups.items()),
          flush=True)
    for name in groups[args.slice]:
        print(f"{args.slice}: {name}", flush=True)
    if args.list_only:
        return
    # Use exact names so neither a substring collision nor a new unassigned test
    # silently changes the bounded shard. cargo/libtest combines filters with OR.
    subprocess.run(CARGO + groups[args.slice] + ["--exact", "--test-threads=1"],
                   check=True)


if __name__ == "__main__":
    main()
