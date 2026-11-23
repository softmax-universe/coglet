# ABI qualification applicability (not final acceptance)

The full frozen wire inventory is 128 cases. `tools/check-abi.py` recursively
checks exact fresh JUnit inventories and zero failures/skips; it uses distinct
runtime-script/archive, actual WAMR admission, and SDK shared-mock routes.
This is not one engine independently executing all pinned guest programs.

| Frozen corpus | Cases | Actual proof / named test | Independent guest replay applicability |
|---|---:|---|---|
| cases.json | 21 | `every_primary_script_reexecutes_independently_with_identical_step_hashes` | Independently supplied data-only mock programs; all per-step hashes compared |
| admission.json | 3 | `admission_instantiates_without_consuming_input_or_calling_handle`, official wire harness | Admission only; zero handle calls, no guest journal to replay |
| manifest-admission.json | 8 | `manifest_catalog_and_grants_are_checked_over_the_wire`, official wire harness | No module execution or guest journal |
| boundary-receipts.json | 46 | `complete_frozen_archive_profiles_validate_without_guest_execution` | Archive consistency, not guest determinism |
| provenance.json | 8 | Same archive test | Provenance checks before any execution; valid recorded journal is not independently reexecuted |
| heard-archive.json | 26 | Same archive test | Runtime-owned boundaries invoke no guest; any recorded reasoning step still needs independent program evidence |
| reference-guests.json | 16 | Official harness via SDK `reference_conformance mock` | Shared SDK/engine role; not runtime journal replay |

All 80 archive requests pin `0061736d01000000`, the empty 8-byte Wasm module,
including call-bearing records. It has no `abi_alloc`, `handle` or memory export.
A real engine cannot reexecute those guest transitions from these pinned bytes.
Feeding recorded calls/output to ObservedTransition establishes host consistency
only; it cannot meet independent guest-program replay acceptance. No frozen
fixture has been edited, no invalid case has been omitted, and this matrix does
not waive the original all-profile gate. The creator must own any resolution
requiring new independently runnable fixtures or a clarified applicability gate.

Additional real-program evidence:
`native_engine_cases::real_wamr_journal_reexecutes_two_steps_with_identical_state_hashes`
runs independently instantiated pinned WAMR code twice and compares the two-step
journal state hashes. It is a runtime replay proof, not all-profile completion.
`replay_checks_state_without_requiring_identical_wall_clock_observations` checks
that independently observed wall durations can differ, while recorded usage must
still be compatible with committed status. Original record checksums and chain
links are validated; deterministic usage/effects/status/snapshots remain compared.

Recovery tests include corrupt retained blobs, effective-budget/boundary attempt
preflight without execution, source highwater forgery, timer reservation gaps and
reuse across a consumed record and pending queue. Authenticated own-source inbox
boundary inputs may advance the source floor; generated wakes cannot invent gaps.

`native_pump_reconciles_retained_actions_but_does_not_dispatch_new_intents_while_fenced`
exercises the actual native-enabled session pump, retained translated bytes,
new-intent refusal (including direct preparation), and untouched queued attempt.
`guest_exit_is_guest_return_abort_including_zero_status` proves typed outcome
classification only. Native WASI/proc_exit execution remains admission-refused.

Remaining: broader pending action/LLM/inbox/dispatch segment-transfer scenarios,
additional malformed trace/replay negatives, independent all-profile replay
resolution, final exact-head independent review, required CI and merge. Internal
WAL history is retained with a 64 MiB frame bound and no reclamation; storage
exhaustion backpressures rather than dropping receipts/lifetime IDs.
