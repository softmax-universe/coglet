# LLM gateway skeleton

This crate implements the trusted, in-process adapter boundary described by
`spec/capabilities/bindings.md` and the frozen ABI. It adds no hub RPC, surface
operation or inter-host protocol. `coglet-mock::llm::MockLlm` implements its provider
interface with strict FIFO scripted requests/replies. No legacy code was ported.

## Embedding contract

1. Only the authenticated runtime outbox worker calls `enqueue`, **after** the
   guest step/request/outbox commit. Construct `Admission` from host identity and
   current `llm.complete` grant, not guest/provider data. This library is not an
   authentication service; possession of its DB/API is administrative authority.
2. Configure a route map outside guest input. Each route maps a logical model to a
   provider and a positive hard cost ceiling in integer micro-units of one currency.
   Providers must enforce that bound (including input/output and ancillary charges).
   Do not admit an unbounded provider/pricing scheme. Budget range is 0..=i64::MAX.
3. Call `dispatch_next` from the worker, not `handle`. The selection closure checks
   current authorization and routes `Call.provider` to the configured adapter
   implementing `Provider::complete`. Provider name is pinned at enqueue. No
   provider is bundled; the closure is the embedding host's dispatch boundary.
   Refused enqueue operations return errors to that host; it records any admission
   failure delivery through its ordinary runtime journal. They never reserve spend.
4. A definitive `Completion` includes a validated ABI reply and actual cost.
   Timeout/cancellation receipts are failed replies, not silent drops. Unknown
   delivery or billing returns `Err`; it keeps the request uncertain and the full
   reservation held. Recovery never resends it. Reconcile using a definitive
   provider receipt and `reconcile`; do not invent a response. Invalid replies or
   costs outside the ceiling likewise retain the reservation for investigation.
5. Terminal reply bytes and spend settle in **one** SQLite FULL transaction before
   delivery. Read with `replay`, construct the `llm.reply` inbox event preserving
   request/correlation IDs, then `record_inbox` with the exact canonical JSONL line
   allocated by the runtime. Deliver those recorded bytes through authorized
   `hub.inbox`; retry with the same message identity. The runtime owns authenticated
   sender binding, global index/seq/ID allocation, durable dedup, boundary mutation,
   `runtime.inbox` wake, and trace export. This store does not impersonate a full ABI
   step/boundary journal. `record_inbox` validates schema, correlation, reply and
   sender/source equality and refuses any later replacement.
6. Offline replay reads only recordings (`replay`/`replay_inbox`), with no provider
   argument. Missing recordings fail closed. Pending recordings survive restart.

SQLite serializes reservation/claim/settlement transactions across connections.
An uncertain claim is durable before I/O; a crash between claim and dispatch can
therefore require manual/provider reconciliation even when no call was sent. This
conservative liveness tradeoff avoids blind duplicate billing. Terminal identities
are retained forever by this skeleton. Use a private local durable database;
backups must include SQLite WAL state. Storage/I/O errors propagate, never trigger
an automatic retry. This is not cross-service exactly-once delivery.

## Verification

`cargo test -p coglet-llm -p coglet-mock` exercises offline exact-byte replay after
reopen, per-coglet sums, reservations, exhausted budget, duplicate IDs, uncertain
recovery, malformed provider receipts and shared-connection claims, using the
scripted provider through the same adapter boundary. These are focused skeleton
integration tests, **not** full language-neutral ABI/runtime conformance or M1
real-provider/crash qualification.

No real-provider call is needed for the deterministic suite. If adding one, start
`~/.hermes/plugins/pool-llm/bin/hermes-pool-llm serve` and use its advertised local
URL via `ANTHROPIC_BASE_URL` (and the proxy's supported credential flow). Never
fall back to a direct provider endpoint or put credentials in fixtures. A real
provider adapter, transport auth, full runtime integration and crash-injection
qualification remain the later full-gateway/M1 phase in `docs/plan.md`.
