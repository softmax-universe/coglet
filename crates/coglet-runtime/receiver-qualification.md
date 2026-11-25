# Runtime receiver qualification

## Scope and current evidence

The runtime serves the frozen device-side `debug.read/write/pause/step`,
`hub.inbox/cancel`, scoped `hub.status`, immutable published `hub.git.fetch`, and
expired `hub.actions` admission through the production authenticated
`DeviceHandler`. The reference peer uses a device-dialed encrypted iroh session;
stdio only bridges bytes between the language-neutral harness and that session.
There are no fixture wire methods, canned replies, or reply-code rewrites.

`python3 crates/coglet-runtime/run_receiver_conformance.py` ran the 25 named cases
below successfully on the Linux native WAMR reference backend, with zero failures
or skips. This is this finite receiver subset, **not full hub conformance**.
`check_receiver_evidence.py` independently revalidated every original-WAL report
and rejected 100 deliberate in-memory evidence corruptions. Those mutation
controls and the five Python helper/custody tests are validator tests, not extra
wire cases. The runner prints the evidence directory and writes JUnit to
`target/hub-receiver-conformance.xml`.

ADR-0032 landed first as PR #162, merge
`d8725dcaf1479555a81af6c6bde89cb8cb8dc6c1`, after independent exact-head review and
required CI through the native merge queue. It changes only the foreign-receipt
case's expected error from `permission_denied` to `not_found`, with ADR/README
clarification. The receiver preserves nondisclosure: foreign and unknown IDs have
identical error objects. Both receipt and original-method grants are present in
the ownership control; no missing-grant bypass is used.

Final whole-workspace/native gates, independent receiver exact-head review,
required CI, receiver PR and native-queue landing remain separate release gates;
local frozen qualification alone does not claim they are complete.

## Frozen requirement-to-case matrix

Every default first completes actual native step zero and durably stops with an
empty router and inbox. Pause profiles then explicitly start; held profiles then
pause and enter a second actual bounded guest. Before the case, the receiver is
retired and its **original** WAL reopened for the baseline, then the same receiver
is reconstructed. After the case, the actual held guest is awaited, session
serving is aborted **and joined**, handler ownership retires, and the original
WAL is reopened again. Only exact lock-contention errors receive a bounded retry.
No still-owned WAL byte-copy fallback and no killed-guest completion inference.

| Frozen cases | Independent persistence / disclosure oracle |
| --- | --- |
| `action-expired` | Original request and negative accept retained; no new dispatch/order/outcome, journal entry, private effect or unrelated state change beyond the executed step-zero baseline. |
| `cross-owner-receipt`, `unknown-receipt` | Exact closed `not_found` error shape, no result/metadata; entire nonclock state and journal unchanged; foreign owner's original applied write, scope, receipt and bytes retained. |
| `debug-inbox-protected`, `debug-traversal`, `read-grant-is-not-write`, `write-stale-snapshot` | Correct path/grant/CAS refusal; committed files, staged boundary, operation IDs, receipts, queues, audit and journal unchanged. Read-only control actually reads its original file. |
| `read-actor-isolation`, `read-ended-ticket`, `read-missing-ticket`, `read-scoped-allowed` | Exact actor/ticket grant in allowed control, actual committed file text and independently recomputed ABI snapshot hash; negatives contain no private result and cause no state change. |
| `pause-single-step`, `single-step-requires-pause` | Actual counter guest commits step one once (`/data/count` changes `1` to `2`), original receipt/audit retained and paused state persists; unpaused refusal admits no operation or execution. |
| `status-actor-isolation`, `status-ended-ticket`, `status-missing-ticket`, `status-scoped-allowed` | Exact allowed scope and full stopped/step-zero/live-commit/pending projection; negatives disclose no status metadata and leave state unchanged. |
| `fetch-actor-isolation`, `fetch-ended-ticket`, `fetch-missing-ticket`, `fetch-scoped-allowed` | Exact granted immutable signed publication metadata; unauthorized scopes disclose none. Observer resets after metadata without credit; retired transfer quarantine entries return to their original baseline. No private live-tree export. |
| `inbox-durable-dedup`, `inbox-conflicting-id` | Exactly one full compact sorted-key ABI JSONL line, original event/sender/ID, index zero and escaped embedded newline; original applied receipt, one staged hub-origin boundary, one terminal audit and one wake. Conflicting retry cannot alter any of them. |
| `inbox-boundary-queued` | Real native execution overlaps queued admission and cancel; original operation becomes cancelled, cancellation's own ID/request/receipt survives reopen; no inbox append after actual guest completion. |
| `queue-backpressure` | 255 actual handler inbox admissions plus the held step; authenticated status observes 256 pending before readiness. Overflow consumes no operation/message/audit/boundary/line custody. After actual guest completion all admitted requests remain bound to receipts/pending state; any materialized lines have exact original provenance and contiguous indices. |

The setup guest returns at step zero and loops over native `memory.fill` at step
one. This keeps the bounded guest alive through queue setup without exhausting
the interpreter's maximum instruction allowance first. The pressure fixture has
an eight-second guest wall bound and delays readiness until 2.5 seconds after
entry, while retaining the case's unchanged two-second wire expectations and the
harness's separate ten-second teardown cap. Failure to observe a genuinely full,
still-running fixture fails setup. No timer or production budget is relaxed.

## Durable implementation and operational regressions

- Queue admission atomically retains original request/intent/scope, receipt and
  audit. Applied file/inbox bytes, wake, staged boundary and terminal receipt are
  one WAL frame; reads use the materialized staged snapshot, not initial files.
  `seal_boundary` is not used per mutation; the journal-derived base is preserved.
- Cancel reserves its own immutable operation ID. Message-ID dedupe compares the
  full persisted delivery without its runtime-assigned index. All receiver,
  lifecycle and Git operations share ID custody and pressure fencing.
- `detached.rs` reserves native execution, pins the original base/manifest/exact
  staged prefix, rechecks original authority at the same-WAL preguest fence, then
  releases SQL and coordinator locks around actual guest execution. The returned
  real observation reconciles into current state, preserving concurrent
  admissions/cancel; unknown outcomes remain fenced and are never blindly retried.
- Installed-push `code_pending`, artifact/restart validation and kill gates apply
  before every guest entry. Integration includes merged PR #152 `c46b214`.
- The runtime-owned weak driver retries at 250ms, backs off transient errors to
  five seconds, and handles at most 16 operations per batch. It skips an already
  owned target instead of queuing another strong handler waiter. Queued inbox
  admissions do not spawn one drain task each. A terminal step returns before
  unrelated inbox materialization; the owned driver continues those batches.
  The blocking worker retains its permit if its awaiting task is cancelled.
- Original mutation authority/path checks remain distinct from an expired RPC
  reply allowance. Idle writes/inboxes return their actual applied receipt;
  in-progress mutations remain durable queued work, not bridge-transformed replies.

Named operational regressions (Rust suites, separate from frozen cases):
`back_channel_real_receiver_write_read_receipt_and_restart`,
`back_channel_real_receiver_scope_and_path_limits`,
`back_channel_real_receiver_inbox_dedup_reopened_wal_oracle`,
`back_channel_pause_is_durable_and_single_step_requires_pause`,
`real_bounded_guest_allows_durable_inbox_cancel_while_running`,
`ordinary_guest_completion_drives_deferred_inbox_without_another_request`,
`back_channel_single_step_runs_real_wasm_once_and_reuses_original_receipt`, and
`idle_durable_admission_outlives_rpc_timeout_without_terminal_failure`.
The last is an explicit trusted-clock fault seam, not frozen-wire qualification.

## Explicit exclusions

- Positive nonexpired `hub.actions` remains unadmitted (`unavailable`). The real
  expired-action rejection is not positive admission, adapter completion, or a
  complete action-outcome/recovery bridge. This card's acceptance names expiry
  only; no full bridge claim is made.
- ADR-0031's plain-filesystem / `manage.*` conversion is separately owned and is
  not implemented here. No new debug/Git control surface is added beyond this
  card's frozen methods. Factory DB/WASI work is untouched.
- Git-push cancellation is outside this card's withdrawn Git-receipt slice.
- This Linux reference proof is not Apple platform qualification.

## Reproduction

```sh
python3 crates/coglet-runtime/run_receiver_conformance.py
python3 -m unittest discover -s crates/coglet-runtime -p test_receiver_oracles.py -v
# Printed original evidence directory; validates reports, never regenerates replies:
python3 crates/coglet-runtime/check_receiver_evidence.py <evidence-directory>
```

The runner owns every escaped implementation child by PID plus Linux start-time
identity in a fresh test-only evidence directory, and cleans it in a finalizer.
The pipe proxy's child has private pipes, not inherited harness stdout/stderr.
Teardown hooks spawn nothing: they await the real child result and exit, validate
reopened reports, fail the case on any missing/negative result, and kill only an
exact still-owned child on failure. This lets the harness clean its proxy process
group immediately after the final reply without killing the actual guest before
its bounded oracle completes. Raw reports contain only disposable fixture data.
