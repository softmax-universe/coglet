# R3 returned-outcome diagnostics

The required `published_r3_runtime_journal_fs_trap_payload_chain` still executes
one unchanged signed publication on each native architecture, requires two
commits and one rolled-back trap, and independently replays its journals. No
budget, success assertion, source identity or native-pair gate is changed here.

## Failure evidence

Immediately after each `RuntimeService::run_next` returns, and before unwrapping
or inspecting its journal, the harness retains the complete returned value (or
the distinct `Ok(None)`/debug-formatted error), input context, step, architecture
and precomputed publication/setup/Wasm SHA-256 pins. Each `step-N.json` is created
exclusively, written and fsynced, then its directory is fsynced. Retention failure
fails the test; existing evidence is never overwritten.

These files live in the sibling `${COGLET_R3_RUNTIME_DIR}-diagnostics`, outside
the unchanged sealed success inventory. Separate `r3-diagnostics-*` artifact
names deliberately do not match the aggregator's `r3-runtime-*` wildcard.
Workflow uploads run even after test failure; an absent directory warns, never
qualifies execution. The Python wrapper additionally retains its original
execution/report-validation exception where possible and always re-raises it,
even if that secondary retention fails. It never seals a failed run.

This captures **returned outcomes**, not native process termination, a call that
never returns, or engine initialization failure. JSON diagnostics are unsealed
human diagnostic evidence, not a substitute for authenticated publication or
successful native-pair qualification. They carry no historical causal inference.

## Historical evidence boundary

Mirror run `45971850324`, ARM job `140415826509`, publication `15184704536`
failed an expected `committed` assertion with actual `aborted`. Its returned
journal/failure kind was not written before that assertion, and the retained
artifact listing contains no ARM runtime output. The original journal is not
recoverable from those retained logs/artifacts. A separate successful publication
cannot establish this failure's cause. New runs of this changed harness collect
new evidence; they do not reconstruct the original observation or erase its
failure.

## Named synthetic controls (not native qualification)

`cargo test --locked -p coglet-hub --test r3_diagnostics_controls` exercises the
same persistence helper: `aborted_outcome_persists_then_original_assertion_panics`,
`err_outcome_persists_debug_string_then_unwrap_panics`,
`none_outcome_persists_then_second_unwrap_panics`,
`existing_diagnostic_is_refused_and_bytes_unchanged`,
`failed_persistence_cannot_pass`,
`sibling_directory_is_not_the_runtime_inventory`, and
`pins_hash_publication_inputs_before_execution`.

`python3 crates/coglet-hub/test_check_r3.py` tests original-exception preservation,
secondary write failure, no sealing after failure, no overwrite, unchanged
success sealing and separation from the mandatory native-pair inventory.
Actual native replay remains the separate, unchanged `r3_runtime` target; a
compile-only invocation or these synthetic controls do not qualify that target.
