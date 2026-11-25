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
  ≤16 concurrent connections, one thread each so `revoke_output_lease` never
  queues behind another request. 3 s per-frame and 10 s idle deadlines.
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

Success: `{"version":1,"ok":true,"method","nonce","principal","actor","target","ticket","daemon_instance","result":{…}}`
Failure: `{"version":1,"ok":false,"nonce":<echo or null>,"error":{"code","retryable"}}`
Codes: `invalid_argument`, `permission_denied`, `conflict`, `resource_exhausted`,
`unavailable`, `unauthenticated`. A failure for a malformed/unauthenticated frame
closes the connection.

- `attest_applied` result: the retained `AppliedGeneration` (policy_state).
- `observe_health` result: the generation-bound sample (policy_state) with
  `output_lease_state` ∈ `closed|enabled|revoked|uncertain`.
- `enable_output_lease` result: `{"lease":"enabled",…,"custody":"retained"}`.
- `revoke_output_lease` result: `{"lease","gate":"closed","in_flight_drained",
  "durable":"recorded|not_required|not_opened|pending|uncertain","daemon_instance"}`.
  The gate is closed before this reply regardless of `durable`.
- `reconcile_emergency_stop` result: `{"state":"stopped"|"none","operation_id"}`;
  receipt bodies are never returned.

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

Vectors: `tests/vectors/policy_local_frames.json` (frame level). Binding and UID
vectors: `policy_local::tests`.
