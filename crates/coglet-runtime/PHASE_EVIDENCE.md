# Bounded native phase failure evidence

This is diagnostic instrumentation, **not** a causal explanation of the historical main reset or the remaining local early-history run11 failure.

`SurfaceListener::with_service` idempotently enables a private recorder on its existing `DurableStore`. Ordinary C ABI/probe-only/nonlistener services have no recorder. Session construction clones the same optional Arc under the original service lock; the original `DetachedRun` carries it outside coordinator locking. Each service retains its own recorder across reconnects. The daemon accept loop is sequential; entries are service-wide aggregate observations, not request/actor attribution. Session start/terminal markers delimit retained boundaries when still present.

The recorder stores latest64 scalar `(monotonic_elapsed_us, phase_id)` entries. Marks use only a nonblocking leaf `try_lock`, fixed array stores and observational `Instant::elapsed`; no formatting, allocation, I/O, payloads, IDs, paths or grants. Contention/poison loses a marker and increments a loss counter. Snapshot unavailability is explicit. Diagnostic timestamps never feed runtime clocks, expiry, deadlines or authorization. WAL/ABI serialized bytes and replay identity do not include the recorder.

Socket errors and existing closed-session terminal returns snapshot before the OUTER disconnect, preserve the existing disconnect calls (including inner invalid-frame/EOF/tick cleanup), and emit only AFTER outer teardown. Clean EOF emits nothing. Output is a single best-effort write from a fixed4096-byte stack buffer with a versioned numeric phase map; no retry/flush and the original Result is returned unchanged. Partial/error output is possible. This does not guarantee stderr latency; process death/forced cleanup may lose the entire ring.

Phases distinguish actual socket read from preceding pump/clock work; supervisor and dispatch; detached preparation, original fence, engine and complete step; reconciliation/serde; WAL encode, write, fsync and cached-state clone. Return means observed return, **not success**, except WAL write/sync done markers follow actual successful calls. Database-specific encode has no separate marker; its common append is visible. Fixed-depth idle chatter can overwrite earlier intervals. Missing/partial/overwritten markers never establish causality or process death. Client timestamps cannot prove queued-byte consumption; retain external scheduler/I/O evidence for any capture.

Named deterministic tests:
- `phase_ring_retains_latest_64_in_order`
- `phase_ring_services_are_isolated_and_contention_is_reported`
- `phase_failure_output_is_bounded_and_preserves_original_error`
- `phase_wal_success_and_real_write_failure_preserve_order_and_state` (real read-only descriptor EBADF, original poison/state preserved; fsync failure not injected)
- `phase_socket_failure_retains_read_and_teardown_order_without_masking` (real authenticated UnixStream pair)
- `phase_detached_ownership_spans_engine_reconciliation_and_wal` (native-engine feature; original detached coordinator path with MockEngine, not real-Wasm qualification)

Actual RED logs precede implementation for ring/reporter/WAL/socket/detached assertions. Feature/native suite and exact-head independent review remain separate gates. Only ONE changed-evidence max20 fail-first diagnostic cohort is authorized after independent exact-head review, with owned/reaped loads and raw evidence. Passing it remains inconclusive for the prior failed process. No quarantine, deadline change, repeated required test, or acceptance amendment is part of this instrumentation.
