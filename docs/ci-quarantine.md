# Flaky-test quarantine

Required checks gate the merge queue. A test that fails at random there ejects its
PR and restarts every PR queued behind it, so a flaky test must leave the required
path quickly, and must also stay owned until it is fixed.

## The list

`ci/quarantine.toml` lists quarantined tests. Each entry has:

- `id`: for conformance, `<implementation>.<suite>/<case>` exactly as `conformance/run`
  prints it; for Rust, the libtest path in its test binary (`name` or `module::name`),
  matched with `--exact`. libtest has no crate filter, so the id skips that exact name in
  every test binary: use a module path that is unique in the workspace. `stability`
  fails if an id names no test.
- For Python codegen wrappers, `[[python]]` uses the exact unittest discovery id
  (`test_go.GoEmitterTests.test_go_module_tests`). `scripts/ci/python-tests.py`
  fails on unmatched ids and skips only those exact tests; direct unittest/Go
  commands remain unfiltered. Stability repeats held Python tests explicitly.
- `card`: the board card that owns the fix.
- `since`: a TOML date.
- `reason`: what failed and the run where it was seen.

`scripts/ci/quarantine.py check` validates the file; the `conformance` job runs it.

## Where each test runs

| Check | Quarantined tests |
|---|---|
| `rust` and `conformance` (required) | skipped in every `cargo test` step (via `scripts/ci/cargo-test`), every registry or ABI `conformance/run` (`--quarantine`), and codegen discovery (`scripts/ci/python-tests.py`). Other Python unit tests are not covered. |
| `stability` (not required; every 6 h, on demand, and when the list changes) | run 20 times, with the full registry also repeated 20 times |

`stability` fails when any entry is older than 14 days, so a quarantine cannot be
forgotten.

## When to quarantine

Only a test that failed a required check and passed on the same tree (a merge-queue
ejection, or a red head that is green on rerun). The Coglet Orchestrator files the
owner card and the quarantine PR together. The PR that makes the test deterministic
removes its entry and shows a green `stability` run (dispatch it on the branch).

Do not quarantine a test that fails consistently: that is a real failure.

## Stress runs are not required checks

Repetition proves stability, so it belongs in `stability`, not in the required jobs,
where 20 repetitions turn a one-in-a-hundred flake into a likely ejection. The
`surface-readiness` stress workflow likewise runs only when surface code changes,
nightly, and on demand, and never in the merge queue.
