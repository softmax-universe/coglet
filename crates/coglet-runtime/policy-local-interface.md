# Local producer interface (ADR-0033 §1) — request/reply schema

Opt-in, trusted-local only. Enabled by a `profile` object in the `--router-service`
config; absent means no socket and no output gate (legacy behavior). No hub,
adapter or guest method is added. Not a frozen-hub conformance claim.

## Transport

- Unix stream socket at `profile.socket` (absolute path). Parent directory must be
  owned by the daemon user with mode `0700`; the socket is `0600`. A live listener
  is never displaced; a refused (stale) socket of the same owner is replaced.
- The kernel peer UID (`SO_PEERCRED` / `getpeereid`) must equal a configured
  principal `uid`; a peer whose UID matches no principal is closed without a read.
- One JSON object per line, `\n`-terminated, at most 65536 bytes (replies too).
  ≤16 concurrent connections, one thread each. 3 s per-frame and 10 s idle
  deadlines. This pool gives `revoke_output_lease` NO liveness under ordinary
  saturation: at capacity a new connection is closed before it is parsed.
- Reserved emergency endpoint (ADR-0043): whenever a `profile` is configured the
  producer also binds `<socket>.revoke` (the literal suffix appended to the
  complete `socket` path; same directory, owner, mode 0600 and UID rule; not a
  profile field). It accepts exactly ONE request per connection and dispatches
  only `revoke_output_lease`, with the unchanged request/reply frames and
  binding rules. Any other authenticated method fails `invalid_argument` with
  its nonce and is never dispatched; malformed, over-size and forged-binding
  frames fail as on the ordinary socket. It has its own 4-connection bound and
  workers: a fifth concurrent connection is closed without a reply, never
  queued and never moved to the ordinary pool. Each connection has 3 s from
  acceptance for its complete request (no idle allowance) and a 3 s reply
  write; it is always closed after one reply attempt.
- A consumer that needs the ADR-0019 emergency guarantee MUST send revoke to
  `<socket>.revoke`. It must not fall back to the ordinary socket and claim
  reserved admission when the sibling is absent or refuses; a producer without
  the sibling is not qualified for that guarantee. Revoke on the ordinary socket
  stays valid with identical gate semantics when it is admitted.
- Startup binds both sockets or neither: both paths are checked (length, type,
  owner, live listener) before either is replaced, and a failure of either bind
  or accept worker fails daemon startup with every output gate closed. Without
  a `profile` neither socket exists.
- While runtime custody is busy, at most ONE deferred revoke recorder exists per
  target across both sockets; further revokes report `durable:"pending"`.
- Closed objects: unknown/missing fields, duplicate keys at any depth (including
  via escapes), non-UTF-8, trailing text, floats/negative integers are refused.

## Profile

```json
{"socket":"/run/cogletd/p/producer.sock",
 "principals":[{"principal":"policy-consumer","uid":1001,
   "actor":"policy-actor","target":"coglet.one","ticket":null,
   "stop":{"actor":"original-stop-actor","ticket":"t-1",
           "operation_id":"<32-hex correlation id>","params":{}}}]}
```

`stop` is the exact original `hub.stop` context (actor, ticket incl. absence,
operation ID, params) retained in the WAL before any lease is granted. The ID must
be a fresh `correlation-id` after each resolved emergency (see Custody).

## Request

```json
{"version":1,"method":"<m>","nonce":"[A-Za-z0-9._:-]{1,128}","principal":"…",
 "actor":"…","target":"…","ticket":null,"params":{…}}
```

`principal/actor/target/ticket` must equal the configured binding for the peer UID
exactly; any mismatch is `permission_denied` with no nonce or identity echo.
Nonces may not repeat on a connection.

| method | params (closed) |
|---|---|
| `attest_applied`, `observe_health` | `generation:u64`, `commit` (40/64 lowercase hex), `executable_sha256` (64 hex) |
| `enable_output_lease` | the three above + `daemon_instance` (64 hex, from any prior reply) |
| `revoke_output_lease`, `reconcile_emergency_stop` | `{}` |

## Reply

Normative schema: `tests/vectors/policy_local_replies.schema.json` (JSON Schema
2020-12; every object closed: unknown, missing, duplicate-key, mistyped, float,
negative or out-of-range members are refused). Replies are one JSON line, at most
65536 bytes. A consumer validates the whole line against the schema AND the
semantic checks below before using any member. Nothing in a reply is accepted from
caller-supplied JSON; only a reply read from the authenticated local connection.

Success envelope (all members required):

```json
{"version":1,"ok":true,"method":"<m>","nonce":"<echo>","principal":"…","actor":"…",
 "target":"…","ticket":null,"daemon_instance":"<64 hex>","result":{…}}
```

`method/nonce/principal/actor/target/ticket` are exact echoes of the request
(`ticket` is explicit `null` when absent); `daemon_instance` is the producer's
per-process identity.

Failure (all members required): `{"version":1,"ok":false,"nonce":<echo|null>,
"error":{"code":"<code>","retryable":<bool>}}`. `code` ∈ `invalid_argument`,
`permission_denied`, `conflict`, `resource_exhausted`, `unavailable`,
`unauthenticated`; `retryable` is `true` exactly for `unavailable` and
`resource_exhausted`. `nonce` is `null` when the frame did not parse or failed
authentication (those close the connection, never echo identity); otherwise it is
the request nonce. No detail text, receipt body or identity is ever disclosed.

Notation: `hexN` = lowercase hex of exactly N chars; `u64` = JSON integer
0..=2^64-1 (no floats, no negatives); `name` = `[A-Za-z0-9._:@-]{1,256}`. Every
member below is required unless stated.

### `attest_applied` result — the retained `AppliedGeneration`

| member | type |
|---|---|
| `version` | `1` |
| `target` | name |
| `router` | hex64 |
| `generation` | u64 |
| `commit` | hex40 (the retained signed `new` commit) |
| `executable_sha256` | hex64 (digest of the active `bin/step.wasm`) |
| `code` | admission (below): the code admission that produced this generation |
| `initial` | `{router: hex64, code: admission, install: {operation_id: hex32, intent: hex64, scope}}` — the ORIGINAL signed code admission and install context |
| `receipt` | `{operation_id: hex32, state: "applied", step: u64, error: null}` |
| `boundary` | `{kind:"install", operation_id: hex32, step: 0}` or `{kind:"code", trace_id: hex32, operation_id: hex32}` |
| `record_offset` | u64 (journal offset of this activation's first attempt) |

- admission = `{signed_ref, operation_id: hex32, intent: hex64, scope}`;
  `signed_ref` = `{statement:{purpose:"hub-ref", hub: hex64, coglet: "coglet:"+hex64,
  ref:"refs/heads/hub", old: hex40|null, new: hex40, archive_sha256: hex64,
  generation: u64}, signer: hex64, signature: hex128}` (the whole signed statement).
- scope = `{actor, method, ticket?}` with `method` ∈ `hub.git.push` (code) /
  `hub.install` (install). The `ticket` member is OMITTED, never `null`, when the
  original had none (this is the only optional member in any reply).
- No `ok`/health/lease claim is carried; a stopped initial install may attest.

### `observe_health` result — the generation-bound sample

`version:1`, `target` name, `generation` u64, `commit` hex40,
`applied_operation_id` hex32 (the attested code admission operation),
`executable_sha256` hex64, `activation` (same shape as `boundary`),
`counters` `{attempted, committed, failed, budget_exceeded}` (u64 each),
`observation_sequence` u64 ≥ 1, `producer_instance` hex64,
`clock` `{supervisor_boot: 1..128 chars, continuous_millis, runtime_anchor: hex64,
logical_millis, runtime_boundary_millis, anchor_continuous_millis,
anchor_logical_millis}` (u64 millis), `admitted_capabilities` (unique
`ns.name` capability names, sorted), `integrity:"verified"`,
`storage:"available"`, `attempt_fence` ∈ `clear|uncertain`, `lifecycle` ∈
`running|stopped|faulted`, `code_pending` bool, `output_lease_state` ∈
`closed|enabled|revoked|uncertain`. Unavailable/unverifiable state is a failure
reply, never a sample; there is no `unavailable` member value. A durably paused
scheduler is unavailable to this interface: the frozen lifecycle enum cannot
represent pause, so it must not be mislabeled `running`. Refusal retains no new
observation sequence, including after WAL reopen.

### `enable_output_lease` result

`{"lease":"enabled","generation":u64,"commit":hex40,"executable_sha256":hex64,
"daemon_instance":hex64,"custody":"retained"}`

### `revoke_output_lease` result

`{"lease":"revoked"|"uncertain","gate":"closed","in_flight_drained":bool,
"durable":"recorded"|"not_required"|"not_opened"|"pending"|"uncertain",
"daemon_instance":hex64}`. The gate is closed before this reply whatever `durable`
is. `lease` is `uncertain` if `in_flight_drained` is `false` or `durable` is
`uncertain` (the schema enforces both); it never reports `closed` or `enabled`.

### `reconcile_emergency_stop` result

Exactly one of `{"state":"none"}` (no retained obligation) or
`{"state":"stopped","operation_id":hex32}`. Fenced/uncertain custody is a failure
reply (`conflict` / `unavailable`), never a success state. Receipt bodies are never
returned.

### Semantic reply checks (not expressible in JSON Schema)

A consumer MUST reject a schema-valid reply unless all of these hold
(`policy_local::reply_tests::semantic`, exercised by the `layer:"semantic"` vectors):

1. Envelope echo: `method/nonce/principal/actor/target/ticket` equal the request;
   a failure `nonce` is `null` or the request nonce.
2. `attest_applied`, `observe_health`, `enable_output_lease`: the result
   `generation`, `commit`, `executable_sha256` equal the request pins; `target`
   (attest/health) equals the request target. For attest additionally
   `code.signed_ref.statement` has `new == commit`, `generation == generation`,
   `coglet == target`, and `hub == signer == router == initial.router`;
   `initial.install.operation_id != initial.code.operation_id`.
   Install boundary: `code == initial.code`, `receipt.operation_id ==
   boundary.operation_id == initial.install.operation_id`, `receipt.step == 0`,
   `record_offset == 0`. Code boundary: `receipt.operation_id ==
   boundary.operation_id == code.operation_id`, `code.operation_id !=
   initial.code.operation_id`, and `generation` exceeds the original admission's.
3. `observe_health`: `committed + failed + (attempt_fence == uncertain ? 1 : 0) ==
   attempted`; `budget_exceeded <= failed`; `logical_millis >=
   runtime_boundary_millis`; `continuous_millis >= anchor_continuous_millis` and
   `logical_millis == anchor_logical_millis + (continuous_millis -
   anchor_continuous_millis)` within 1 ms; `applied_operation_id ==
   activation.operation_id` iff `activation.kind == "code"`.
4. `enable_output_lease`/`revoke_output_lease`: `result.daemon_instance` equals the
   envelope `daemon_instance`; enable also equals the request's `daemon_instance`.
5. Ordering: the consumer additionally requires `observation_sequence` to increase
   for one `producer_instance`, and the `activation`/counters domain to match the
   expected activation (consumer policy; the producer never decides promotion).

## Custody rules

- Every gate starts `closed` at process start; a revoked/uncertain gate cannot be
  re-enabled in that process.
- Enable requires: running coglet, exact generation/commit/executable pins,
  current `daemon_instance`, kill switch clear, fresh stop and start authority,
  and the stop obligation durably retained in the managed WAL BEFORE the grant.
  Unresolved custody (retained by an earlier process, stop-pending, or a reused
  operation ID) refuses enable until `reconcile_emergency_stop` applies the
  original stop.
- Output is exposed only inside `OutputGate::publish`; revoke waits for at most
  one already-started bounded write, so nothing is newly exposed after it returns.

Vectors (all shared by producer, consumer and the black-box harness):

- `tests/vectors/policy_local_frames.json` — request frames; its
  `reserved_endpoint` list gives the `<socket>.revoke` outcome of each frame.
- `tests/vectors/policy_local_replies.schema.json` — closed reply schema.
- `tests/vectors/policy_local_replies.json` — reply vectors: each pairs the exact
  `request` frame with a `reply`; positives for all five operations and every
  failure code, `layer:"schema"` negatives (must fail the schema) and
  `layer:"semantic"` negatives (schema-valid, must fail the semantic checks),
  plus raw reply lines (duplicate keys, trailing text, non-object, empty).
  Over-size (>65536 byte) replies are generated, not stored.
- Binding and UID vectors: `policy_local::tests`; reply conformance:
  `policy_local::reply_tests` (`assert_result_conforms` lets integration tests
  check results from the real producer against the same schema).
