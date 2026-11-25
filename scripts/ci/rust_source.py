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
# PR171 first requires the production selected-profile peer in this workflow.
# Later workflow generations must not demand these inputs from older exact heads.
PROFILE_BOUNDARY = "ebad856289393a2a770478a038fe12296bc29385"
PROFILE_INPUTS = (
    "crates/coglet-runtime/tools/profile_fixture_bridge.py",
    "crates/coglet-runtime/tools/test_profile_fixture_bridge.py",
    "conformance/impls-surface02-runtime.toml",
    "conformance/tests/test_expect_close.py",
)


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


def profile_required(repo, selected, *, boundary=PROFILE_BOUNDARY):
    """Classify the actual selected source, never the mutable base or file absence."""
    commit(repo, boundary)
    commit(repo, selected)
    # A locally present boundary object does not make truncated ancestry complete.
    if git(repo, "rev-parse", "--is-shallow-repository") != "false":
        raise ValueError("profile generation requires complete source ancestry")
    if not ancestor(repo, boundary, selected):
        return False
    for path in PROFILE_INPUTS:
        entry = git(repo, "ls-tree", selected, "--", path).split()
        if len(entry) != 4 or entry[0] not in ("100644", "100755") or entry[1] != "blob" or entry[3] != path:
            raise ValueError(f"required selected-profile input is not a regular blob: {path}")
    return True


if __name__ == "__main__":
    sha, scope = select_source(Path.cwd(), os.environ["SOURCE_EVENT"], os.environ["SOURCE_RUN_SHA"], os.environ.get("SOURCE_PR_HEAD", ""))
    required = profile_required(Path.cwd(), sha)
    profile_status = "required" if required else f"not applicable (selected source predates {PROFILE_BOUNDARY}); not executed or qualified"
    print(f"Required Rust source: {sha} ({scope})")
    print(f"Selected-profile qualification: {profile_status}")
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write(f"sha={sha}\nscope={scope}\nprofile-required={str(required).lower()}\nprofile-boundary={PROFILE_BOUNDARY}\n")
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
        summary.write(f"## Required Rust source\n\n- Event SHA: `{os.environ['SOURCE_RUN_SHA']}`\n- PR head: `{os.environ.get('SOURCE_PR_HEAD', '')}`\n- Actual selected checkout: `{sha}`\n- Scope: **{scope}**\n- Selected-profile generation boundary: `{PROFILE_BOUNDARY}`\n- Selected-profile qualification: **{profile_status}**\n\nBoth native architectures and the Rust aggregate use this SHA. Merge-integration-only artifacts do **not** qualify the PR head for transform publication. Manifest checkout identities and the publication verifier remain authoritative.\n")
