# Runtime-side native qualification

Scope: runtime-side surface state machine, durable outbox and Linux operational
service. This is not full ABI/M1, iOS, acoustic, provider-service or surface SDK
qualification. Frozen specs/cases are unchanged. Independent review and required
exact-head CI remain separate landing gates recorded on PR #53.

## Original acceptance matrix

| Runtime requirement | Executable evidence |
| --- | --- |
| Frozen surface corpus, runtime under test, BOTH transports, zero skips | `tools/check-surface.py` stages every unchanged `peer_role=surface` case and verifies fresh JUnit names/counts. Two negotiation cases per real socket/C ABI transport, four passes, zero skips. The nine `peer_role=runtime` cases test a **surface**, not this runtime. Negotiation alone is not operational qualification. |
| Catalog/grants/dispatch | `tests/operational_surface.rs`: `operational_catalog_grants_translate_and_kill_fences_new_actions`, `missing_mapping_rejects_act_and_invalid_sibling_payload_has_no_authority`, `duplicate_owner_or_wrong_catalog_uri_does_not_install_grants`, `durable_grant_epoch_survives_reconnect_and_restart`. |
| Private events/reads | Complete operational target covers epoch/lease, selected schemas, caller scoping, bounded queues, late replies, source highwaters, atomic enqueue and revoke/expiry/disconnect/reopen purge. Native target's `ffi_private_capture_filters_late_text_and_persists_end_fence` and socket counterpart cross actual transports and read back WAL state. |
| Translate once, durable outbox, identical retry after clock advance, reconnect reconciles rather than duplicates | Native `ffi_byte_identical_retry_after_clock_advances_and_reconnect_no_duplicate_act` and socket counterpart: old envelopes arrive without fresh grants after close/crash and WAL reopen; backend ledger has no second act. Library `byte_identical_retry_after_clock_advances` and `reconnect_reconciles_old_request_instead_of_duplicating_act` inspect persisted dispatch. |
| Authenticated outcomes/receipts | Same native retry tests cover accept, terminal and fresh-sequence duplicate deliveries; reopened WAL has one mutation/wake. `outcome_requires_saved_admission_and_exact_authenticated_source` rejects unknown admission and wrong source/correlation. Accept is not physical success. |
| Production cogletd/C embedding guest execution | Feature-native `ffi_real_wasm_commits_call_order_before_dispatch_and_reconciles` and socket counterpart execute real WAMR, inspect committed call-order journal/acts and reconcile reopen. Both trap counterparts prove rollback and one-time consumption. `native_dispatch_rechecks_clock_after_guest_outlives_grant` admits no fresh acts after real initializer work expires grants. Feature-free service is outbox-only and refuses requested scheduling. |
| Heard before reasoning | `ffi_owner_heard_reads_materialize_before_reasoning` and socket counterpart; revoked-reply/reverse-read variants. Feature mode really executes Wasm and checks boundaries precede consumed observations. Default checks trailing/zero-step state, not execution. `heard_barrier_blocks_reasoning_then_expires_without_a_reply_or_transport_tick` checks independent expiry. |
| Runtime playback defense | `ffi_playback_duck_pair_and_terminal_binding` and socket counterpart cover duck/start/barge/end/stop; provider-native owner counterparts check session/item/read and call dedup. Recovery independently validates durable metadata. Playback units cover malformed recovery, stream/chunk association, pairing, snapshot persistence and terminal freeze. |
| Pointer/framing/lifecycle | Launcher compiles/runs `tests/native_surface.c`: unsupported ABI, null/size/short/exact buffers, copied input, bounded-send/no effect, malformed input, would-block, close and health. Service-native tests close with pending committed work/private observations and reopen WAL. Socket tests cover UID/path/fragments/EOF/inactivity. |

Native operational and library targets complement the small frozen corpus; they
are not represented as extra language-neutral conformance cases. Run complete
targets without filters or ignores.

## Ownership and supported bounds

- Runtime owns pre-execution authorization, immutable ABI intent, translated
  envelope durability, authenticated terminals/inbox dedup, private queues and
  heard boundaries. Surface owns final admission, physical stop/duck, native I/O
  cancellation, uncertain-effect recovery and its durable receipt ledger. The
  deterministic test backend proves runtime reconciliation, not acoustic success
  or exactly-once physical effects. Killing cogletd is not a device test.
- Native clock is Linux boot-ID plus CLOCK_BOOTTIME. Changed boot, rollback,
  unanchored old dispatch, unsupported platform and poisoned WAL fail closed.
  Actual C pointers are exercised on Linux; iOS clock/build/suspension/background
  integration is not qualified by these tests.
- Service requires trusted immutable binding/setup, private persistent WAL and
  pinned Wasm/provenance/manifest plus optional `native_schedule`. It is an
  operational supervisor-configured daemon, not signed-archive admission, hub
  lifecycle service or disconnected background scheduler.
- Restricted WASI preview1 file preopens are hosted on the coglet-fs overlay.
  Clock/random/poll and symlink/hard-link imports still fail admission. Named
  evidence: `native_sdk_guest_writes_data_commits_and_reopens`,
  `native_guest_abort_discards_wasi_writes`,
  `native_wasi_crash_between_journal_and_commit_recovers_one_state`,
  `native_wasi_replay_yields_identical_hashes`,
  `native_forbidden_ops_denied_with_permission_errno`,
  `native_forbidden_imports_fail_admission_without_instantiation`,
  `native_wasi_append_flag_does_not_grant_write_or_renumber_clobber`, and
  `native_wasi_file_growth_reserves_host_bytes_before_allocation`.
  File growth that is not copied from guest memory (`fd_filestat_set_size`
  extension and positional/seeked writes past EOF) is charged to the step's
  `host_bytes` allowance before coglet-fs allocates it; over budget the step
  aborts with no state change.
  The SDK persistence test runs the compiled counter through NativeEngine in
  two fresh runtime subprocesses: the second reads and increments the first's
  committed `/data/count`. The crash test truncates the actual atomic WAL frame
  at header/body/end boundaries; incomplete commits retain the original queue
  and attempt fence, while the complete frame publishes files and consumes it.
  Full ABI/native replay, trace import/export and broader recovery reconstruction
  belong to serial owner `t_01a2085cca051179`. The primary ABI launcher remains a
  21-case subset.
- Capture/playback/provider tombstones have explicit non-evicting 4096-entry
  lifecycle bounds; streams admit at most 64 observed chunks. Reads share 128
  outstanding/completed slots and 4096 per-link correlation tombstones. Exhaustion
  fails closed; indefinite service/compaction is not qualified.
- Frozen playback has no session field. Unseen provider items require the sole
  historically prepared successful current-epoch session; subsequent opens make
  attribution ambiguous and fail closed. This restricts availability rather than
  rebinding delayed callbacks to a new session. Unrestricted provider reconnect
  is not qualified; see `live-heard.md`.

## Reproduction and results

Executed after merging current main, with CMake available:

```sh
cargo test -p coglet-runtime --features native-engine --locked -- --test-threads=1
cargo test --workspace --locked
cargo clippy -p coglet-runtime --all-targets --locked -- -D warnings
cargo clippy -p coglet-runtime --all-targets --features native-engine --locked -- -D warnings
cargo fmt --all -- --check
python3 crates/coglet-runtime/tools/check-primary-abi.py
python3 crates/coglet-runtime/tools/check-surface.py
```

All commands passed. Native target: feature 23, default 17; operational library
21; runtime units 13, all zero ignored. Primary ABI 21/21; surface negotiation
4/4 plus C pointer probes, zero skips. The workspace has one existing deliberately
ignored filesystem subprocess; do not call the whole workspace a zero-skip gate.
CI/review/merge are not inferred from local results.
