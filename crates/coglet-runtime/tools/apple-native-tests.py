#!/usr/bin/env python3
"""Exhaustive Apple native-test partition; no test-name skip filters.

The Factory integration executable dominates the Apple wall budget. Discover its
actual libtest inventory, retain sorted round-robin membership, isolate the
measured baseline-comparison outlier, and split only original shard 0 across
two bounded jobs. All other native targets retain their Cargo execution environment.
"""
import argparse
import json
import subprocess

ROUND_ROBIN_SHARDS = 3
SHARDS = ROUND_ROBIN_SHARDS + 2
ISOLATED = "factory_research::candidate_baseline_comparison_is_reproducible"
PACKAGE = "coglet-runtime"
FACTORY = "factory_database"
BASE = ["cargo", "test", "--locked", "-p", PACKAGE, "--features", "native-engine"]


def runtime_targets(metadata):
    packages = [p for p in metadata["packages"] if p["name"] == PACKAGE]
    if len(packages) != 1:
        raise ValueError("expected exactly one runtime package")
    package = packages[0]
    # Match Cargo's default + native-engine feature closure. Explicit target
    # selection otherwise incorrectly selects opt-in m1-tests/echo-smoke targets
    # that the original unfiltered Cargo invocation does not execute.
    enabled = {"default", "native-engine"}
    pending = list(enabled)
    while pending:
        for feature in package["features"].get(pending.pop(), []):
            if feature in package["features"] and feature not in enabled:
                enabled.add(feature)
                pending.append(feature)
    args = []
    factory = 0
    for target in package["targets"]:
        if not target["test"] or not set(target.get("required-features", [])).issubset(enabled):
            continue
        kinds = target["kind"]
        if kinds == ["test"] and target["name"] == FACTORY:
            factory += 1
            continue
        if "lib" in kinds or "rlib" in kinds:
            args.append("--lib")
        elif kinds == ["bin"]:
            args += ["--bin", target["name"]]
        elif kinds == ["test"]:
            args += ["--test", target["name"]]
        elif kinds == ["example"]:
            args += ["--example", target["name"]]
        else:
            raise ValueError(f"unclassified test target: {target}")
    if factory != 1 or not args:
        raise ValueError("missing Factory or remaining runtime test targets")
    return args


def factory_names(output):
    names = []
    summary = None
    for line in output.splitlines():
        if line.endswith(": test"):
            names.append(line.removesuffix(": test"))
        elif line.strip():
            # libtest's explicit total guards truncated/unrecognized inventories.
            import re
            match = re.fullmatch(r"(\d+) tests?, (\d+) benchmarks?", line)
            if not match or summary is not None or int(match[2]) != 0:
                raise ValueError(f"unexpected libtest inventory line: {line}")
            summary = int(match[1])
    if len(names) < SHARDS or len(set(names)) != len(names) or summary != len(names):
        raise ValueError("incomplete, duplicate or too-small Factory inventory")
    return sorted(names)


def partition(names, shard):
    if shard not in range(SHARDS):
        raise ValueError("invalid shard")
    if names.count(ISOLATED) != 1:
        raise ValueError("missing or duplicate isolated Factory test")
    # Filter AFTER indexing the full inventory: original shard 0/2 membership
    # must not reshuffle just because the outlier moved to its own job.
    if shard == ROUND_ROBIN_SHARDS:
        selected = [ISOLATED]
    else:
        # Split only original shard 0 by its full-inventory position.
        # Do not reindex after excluding the comparator or reshuffle 1/2/3.
        selected = [name for index, name in enumerate(names)
                    if name != ISOLATED and
                    ((index % 6 == 0 and shard == 0) or
                     (index % 6 == 3 and shard == 4) or
                     (index % 3 == shard and shard in (1, 2)))]
    if not selected:
        raise ValueError("empty Factory shard")
    return selected


def run(command, capture=False):
    print("+ " + " ".join(command), flush=True)
    return subprocess.run(command, check=True, text=True,
                          stdout=subprocess.PIPE if capture else None).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["runtime", "factory"])
    parser.add_argument("--shard", type=int, choices=range(SHARDS))
    args = parser.parse_args()
    if args.mode == "runtime":
        if args.shard is not None:
            parser.error("runtime mode has no shard")
        metadata = json.loads(run(["cargo", "metadata", "--locked", "--no-deps",
                                   "--format-version", "1", "--features",
                                   "coglet-runtime/native-engine"], capture=True))
        # Unfiltered cargo test also compile-checks examples with test=false.
        # Preserve that coverage separately from executable target selection.
        run(["cargo", "build", "--locked", "-p", PACKAGE, "--features", "native-engine", "--examples"])
        run(BASE + runtime_targets(metadata) + ["--", "--test-threads=1"])
        run(BASE + ["--doc", "--", "--test-threads=1"])
    else:
        if args.shard is None:
            parser.error("factory mode requires --shard")
        command = BASE + ["--test", FACTORY]
        names = factory_names(run(command + ["--", "--list"], capture=True))
        selected = partition(names, args.shard)
        print(json.dumps({"inventory": names, "shard": args.shard,
                          "shards": SHARDS, "selected": selected}), flush=True)
        # Multiple exact libtest filters are OR-ed, not substring exclusions.
        run(command + ["--", "--exact", "--test-threads=1"] + selected)


if __name__ == "__main__":
    main()
