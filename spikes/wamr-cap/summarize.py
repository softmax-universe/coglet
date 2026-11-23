#!/usr/bin/env python3
"""Recompute nearest-rank measurements and verify complete sample counts."""
import csv
import gzip
import json
import math
import pathlib
import sys
from collections import defaultdict
from typing import Any


def stats(values):
    values = sorted(values)
    return {"n": len(values), "p50_ns": values[math.ceil(len(values) * .50) - 1],
            "p99_ns": values[math.ceil(len(values) * .99) - 1],
            "max_ns": max(values), "mean_ns": sum(values) / len(values)}


def summarize(directory):
    result = {}
    for mode in ("interp", "aot", "aot-shared"):
        path = directory / f"{mode}.csv"
        opener = open
        if not path.exists():
            path = directory / f"{mode}.csv.gz"
            opener = gzip.open
        groups = defaultdict(list)
        with opener(path, "rt", newline="") as source:
            for row in csv.DictReader(source):
                groups[row["kind"]].append({k: int(row[k]) for k in ("index", "value_ns", "detail")})
        expected = {"fresh": 10000, "paced_10hz": 100}
        if mode != "aot":
            expected["deadline"] = 50
        if mode == "interp":
            expected.update({f"fuel_{budget}": 20 for budget in (1000, 10000, 100000)})
        assert set(groups) == set(expected), (mode, set(groups), set(expected))
        for kind, count in expected.items():
            assert len(groups[kind]) == count, (mode, kind, len(groups[kind]))
            assert [r["index"] for r in groups[kind]] == list(range(count)), (mode, kind)
        measurements: dict[str, Any] = {kind: stats([r["value_ns"] for r in rows]) for kind, rows in groups.items()}
        for budget in (1000, 10000, 100000):
            key = f"fuel_{budget}"
            if key in groups:
                progress = {r["detail"] for r in groups[key]}
                assert progress == {budget // 4}, progress
                measurements[key]["identical_progress"] = progress.pop()
        if "deadline" in groups:
            measurements["terminate_to_return"] = stats([r["detail"] for r in groups["deadline"]])
        measurements["paced_completion_lag"] = stats([r["detail"] for r in groups["paced_10hz"]])
        measurements["missed_100ms_slots"] = sum(r["detail"] >= 100_000_000 for r in groups["paced_10hz"])
        measurements["duty_at_10hz_pct"] = measurements["fresh"]["mean_ns"] * 10 / 1e7
        # These deterministic observations must match the assertion logs too.
        log = (directory / f"{mode}.txt").read_text()
        for text in ("undersized_cap_4_CLAMPED_UP initial_pages=5 growth_denied=true",
                     "memory initial_pages=5 cap_pages=8 grow_past_cap=-1",
                     f"limit_1_finite_call_traps={str(mode == 'interp').lower()}"):
            assert text in log, (mode, text)
        fresh = measurements["fresh"]
        assert f"p50_ns={fresh['p50_ns']} p99_ns={fresh['p99_ns']} max_ns={fresh['max_ns']}" in log
        result[mode] = measurements
    negative = (directory / "aot-negative-control.txt").read_text()
    assert "negative_control: terminate sent after 10ms" in negative
    assert "external_kill_exit=137" in negative
    return result


if __name__ == "__main__":
    print(json.dumps(summarize(pathlib.Path(sys.argv[1])), indent=2, sort_keys=True))
