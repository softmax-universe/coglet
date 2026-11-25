#!/usr/bin/env python3
"""Validate actual peer WAL reports; mutation controls are not wire qualification."""
import argparse
import copy
import json
from pathlib import Path

from receiver_oracles import validate
from run_receiver_conformance import CASES


def check(root, selected=CASES):
    root = Path(root)
    reports = {p.name.removesuffix(".result.json.oracle.json"):p
               for p in root.glob("*.result.json.oracle.json")}
    if set(reports) != set(selected):
        raise AssertionError(f"oracle names differ: {sorted(reports)}")
    mutants = 0
    for case in sorted(reports):
        result = json.loads((root / (case + ".result.json")).read_text())
        if result != {"ok":True,"error":None}:
            raise AssertionError(f"{case}: child oracle failed: {result}")
        d = json.loads(reports[case].read_text())
        if d["case"] != case:
            raise AssertionError("oracle file/case mismatch")
        validate(d)
        # Real evidence is retained unchanged. These deliberate in-memory
        # corruptions prove validators reject effects/disclosure, not just wire
        # code matches. They are not fabricated implementation responses.
        for mutation in (
            lambda x: x["records"][0].update({"status":"aborted"}),
            lambda x: x["state"]["dispatches"].update({"unexpected":{"state":"prepared"}}),
            lambda x: x["state"]["runtime"].update({"unresolved_attempt":{"unknown":True}}),
            lambda x: x["observations"].pop(),
        ):
            changed = copy.deepcopy(d)
            mutation(changed)
            try:
                validate(changed)
            except (AssertionError, KeyError):
                mutants += 1
            else:
                raise AssertionError(f"{case}: validator accepted a corrupted oracle")
    summary = {"cases":sorted(reports),"passed":len(reports),"failed":0,"skipped":0,
               "rejected_evidence_mutants":mutants}
    (root / "verified.json").write_text(json.dumps(summary, indent=2) + "\n")
    return summary


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    print(json.dumps(check(args.directory), indent=2))
