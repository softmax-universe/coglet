# Installed router push qualification

This implements ADR-0026's installed-target boundary without new wire methods.
Pre-install completion remains quarantine-only and cannot activate a coglet.

## Custody and ordering

Push admission holds the authority SQL writer transaction, waits for the running
step mutex, and durably fences next-step admission in the runtime WAL **before**
committing the queued SQL operation. Async artifact revalidation therefore cannot
permit another old-code step. A failed SQL admission can leave a conservative
fence; reconciliation resamples queued SQL custody under `BEGIN IMMEDIATE` before
clearing it. Restart retains that fence. No guest runs in reconciliation.

The application callback, still under SQL writer custody, takes the runtime step
mutex, checks original intent/scope and any existing terminal WAL receipt, then
rechecks exact current ref, retained bundle bytes, grant generation, signed device
delegation and manifest grants. It consumes previously staged debug/inbox changes
before cutting code. A new trace pins the replacement code while carrying private
files, pending work, logical time and source floors. Generation must advance;
rollback is another signed generation, never a ref rewind.

The new runtime state, trace custody and original-ID terminal receipt commit in
one WAL transaction before the in-memory native engine is replaced. SQL mirrors
that receipt afterward. An interrupted mirror is reconciled from the original WAL
receipt, even if authority subsequently changes; no confirmed effect is repeated.
Deterministic qualification/authority errors get failed original-ID receipts;
uncertain storage/validator failures retain queued custody and the execution fence.
If the bounded WAL receipt ledger is already full, the callback first checks for
an existing WAL receipt, then refuses **before any effect**. Only this explicit
no-effect `resource_exhausted` refusal is terminalized in the original SQL ledger
without another WAL receipt; SQL commit failure leaves it safely repeatable.
The recovery driver drains successive bounded batches, not only the first 128.

## Requirement-to-test matrix

| Requirement | Named test and real boundary |
|---|---|
| Finish current step before code cut | `installed_push_applies_at_step_boundary`: production router receiver, held step mutex; feature mode executes WAMR before releasing the step |
| No next-step pinning during pending revalidation | `installed_push_original_id_queued_then_applied`: second-validator barrier after durable admission; direct `run_next` refuses while queued |
| Durable next-step fence | `installed_code_pending_fences_next_step_across_reopen` |
| Empty materialized stage and retained source reservations | `installed_code_empty_stage_preserves_source_reservations_across_reopen` |
| Receipt pressure never strands execution behind a queued push | `installed_code_receipt_pressure_is_no_effect_and_preserves_stop_slot`, proto `installed_receipt_pressure_terminalizes_original_sql_id_after_failed_commit`: no code effect; failed SQL update stays queued, retry terminalizes original ID, reopen never invokes application again and queued-fence sample clears |
| Preserve private data | `installed_push_preserves_private_data`: mock writer by default; real restricted WASI writer with native-engine; old/new archive validation and WAL reopen |
| Replace actual native module, deterministic refusal | `installed_code_replaces_real_native_engine_and_terminalizes_unqualified_module`: refuses foreign import, then executes changed Wasm using the service's replacement engine and checks journal hash/compute |
| Original ID queued then terminal and retained | `installed_push_original_id_queued_then_applied`: queued read before validator release, terminal direct response, identical original-ID terminal reads over handler reopen |
| Crash between SQL admission and WAL; crash between WAL and SQL mirror | `installed_push_crash_reconciles_exactly_once`: SIGKILL real cogletd twice, test-local SQLite mirror interruption, original receipt and exactly one code-cut WAL segment; identical retry leaves WAL bytes unchanged |
| Fresh authority failure | `installed_push_stale_authority_fails_closed`: delegation expiry, revocation and generation replacement after queued admission; no live-commit change, immutable failed receipt |
| Multi-batch recovery | `installed_push_drains_more_than_one_recovery_batch`: test-local 131-row recovery backlog using a genuine verified bundle; every ID terminalized |
| Empty stopped cuts, original root binding, no private overlay/rewind | `installed_code_multiple_stopped_cuts_keep_empty_segments_and_root_pin`, `installed_code_cut_refuses_private_overlay_stale_generation_and_expired_gate` |

Run both full default and native-engine runtime package suites, both strict clippy
profiles, fmt and the unchanged frozen ABI/surface checkers. These tests are Linux
runtime/daemon/WAMR evidence, not Apple platform or physical device qualification.
The backlog fixture exercises recovery rows, not 131 independent network uploads.

## Finite bounds

The original lifecycle receipt ledger remains non-evicting (4096 slots, one
reserved for stop). WAL state frames retain all code-transition custody and are
bounded by the existing 64 MiB frame cap; no history reclamation is introduced.
Storage pressure or an uncertain attempt fences new application rather than
dropping receipts or retrying a guest. This change does not add daemon automatic
resume authority, new cancellation behavior, or a new debug/inbox receiver.
