#!/usr/bin/env python3
"""Fail closed if any WAMR platform partition is missing or not successful."""
import json
import os

REQUIRED = {"apple-runtime", "apple-factory", "apple-release-smoke", "ios-runtime", "armv7-qemu"}


def accepted(needs):
    return (isinstance(needs, dict) and set(needs) == REQUIRED
            and all(isinstance(row, dict) and row.get("result") == "success"
                    for row in needs.values()))


if __name__ == "__main__":
    needs = json.loads(os.environ["NEEDS_JSON"])
    print(json.dumps(needs, sort_keys=True))
    raise SystemExit(0 if accepted(needs) else 1)
