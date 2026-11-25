#!/usr/bin/env python3
"""Select one immutable source for required Rust/native integration checks.

Legacy PRs get merge-result coverage, NOT publication qualification of their head.
Never infer compatibility from missing files: modern source deletion must fail in
its own exact checkout, not opt into another tree. See docs/ci-rust-source.md.
"""
import os
from pathlib import Path
import re
import subprocess

# First reviewed exact-head qualification contract (PR159). This is an ancestry
# boundary, not a moving branch name or an existence probe of PR-controlled files.
CONTRACT = "87341a29b3716d3e147df041cfdf4df0507a74d8"


def git(repo, *args):
    return subprocess.run(["git", "-C", str(repo), *args], check=True, text=True, capture_output=True).stdout.strip()


def commit(repo, sha):
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("source identity must be a full immutable commit SHA")
    if git(repo, "rev-parse", "--verify", sha + "^{commit}") != sha:
        raise ValueError("source identity did not resolve to the requested commit")
    return sha


def ancestor(repo, older, newer):
    result = subprocess.run(["git", "-C", str(repo), "merge-base", "--is-ancestor", older, newer])
    if result.returncode not in (0, 1):
        result.check_returncode()
    return result.returncode == 0


def select_source(repo, event, run_sha, head_sha="", *, contract=CONTRACT):
    commit(repo, contract)
    commit(repo, run_sha)
    selected, scope = run_sha, "exact-head"
    if event == "pull_request":
        commit(repo, head_sha)
        parents = git(repo, "show", "-s", "--format=%P", run_sha).split()
        if len(parents) != 2 or parents[1] != head_sha:
            raise ValueError("event merge must have the exact event PR head as second parent")
        if ancestor(repo, contract, head_sha):
            selected = head_sha
        else:
            scope = "merge-integration-only"
    if not ancestor(repo, contract, selected):
        raise ValueError("selected source predates the native qualification contract")
    return selected, scope


if __name__ == "__main__":
    sha, scope = select_source(Path.cwd(), os.environ["SOURCE_EVENT"], os.environ["SOURCE_RUN_SHA"], os.environ.get("SOURCE_PR_HEAD", ""))
    print(f"Required Rust source: {sha} ({scope})")
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write(f"sha={sha}\nscope={scope}\n")
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
        summary.write(f"## Required Rust source\n\n- Event SHA: `{os.environ['SOURCE_RUN_SHA']}`\n- PR head: `{os.environ.get('SOURCE_PR_HEAD', '')}`\n- Actual selected checkout: `{sha}`\n- Scope: **{scope}**\n\nBoth native architectures and the Rust aggregate use this SHA. Merge-integration-only artifacts do **not** qualify the PR head for transform publication. Manifest checkout identities and the publication verifier remain authoritative.\n")
