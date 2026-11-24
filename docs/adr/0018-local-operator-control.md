# ADR-0018: Local operator control of the reference hub

- Status: Proposed (accepted when the independently reviewed PR merges)
- Date: 2026-11-24
- Authors: Coglet contributors
- Related PR: PR containing this ADR
- Supersedes / superseded by: None

## Context

The M2 CLI has no producer for `coglet deploy`. The
[hub protocol](../../spec/hub-protocol.md) defines hub-to-device installation,
start, stop, status and Git transfer, but not how an operator asks the hub to
perform them. Its 0.1 method set is closed; grant management is operator
configuration, not a remotely self-authorizing method. The
[build plan](../plan.md#ground-rules-implementation-context-every-card-follows-these)
requires a recorded decision before filling that gap.

## Decision

### Local authority, not another hub wire method

The reference hub MUST listen on `<state-dir>/operator.sock`, an AF_UNIX stream
socket owned by the hub service user with mode **0600**. The state directory MUST
be owned by that user and inaccessible to other users (0700). Creation must not
follow symlinks or replace an arbitrary existing filesystem entry. On every
accepted connection, obtain OS-authenticated peer credentials (for example
SO_PEERCRED on Linux or getpeereid on macOS) and require the peer's effective UID
to equal the service UID. If credentials cannot be verified, fail closed. The
client likewise checks the server UID. Socket permissions alone are insufficient.
There is no root exception in this interface; an administrator must explicitly
assume the service identity using the host's existing access controls.

Nothing is exposed over iroh, TCP, a socket proxy or the device's socket. There
is no new ALPN, no change to the frozen 0.1 method set, and no new remotely
self-authorizing grant operation. The CLI's `deploy`, `push`, `status` and `stop`
execute on the hub host, directly or as a CLI process launched over SSH on that
host. SSH authentication and authorization stay with the host. Do not forward
the socket to another machine. A remote operator API is deferred to a future ADR.

The server derives audit identity from the authenticated local UID, never an
`actor` field supplied by the request. Record UID and available PID/GID as local
audit metadata; a username is only a display annotation. Multiple people assuming
one service UID are intentionally indistinguishable at this seam; retain SSH/host
access audits separately, without pretending they authenticate a request field.
For downstream requests, map that UID through explicit local provisioning to a
common actor identity and its non-ticket grants. Missing mapping fails closed.
Never borrow kernel authority or invent an actor ID from a UID. Recheck grants,
delegation, generation, expiry, gates and kill switch at their existing boundaries.

### Operations

The local message vocabulary is defined by
[operator.schema.json](local-operator/operator.schema.json). It imports existing
identity, error, receipt and status definitions without extending their wire
unions. This schema describes a reference-hub admin contract, not generated hub
protocol messages; it lives outside `spec/messages/` and its frozen registry.

| Local operation | Meaning |
|---|---|
| `deploy` | Pin source commit and locally provisioned build recipe, build/sign with the hub builder; `hub.git.push` (the protocol's Git push), then `hub.install` with the provisioned coglet-key delegation, then `hub.start`. |
| `push` | Same hub build/sign and verified `hub.git.push`, but no install or start. |
| `stop` | Invoke `hub.stop` and wait for its safe-boundary receipt. |
| `status` | Obtain `hub.status`, returning its bounded observation plus a local operation receipt. |
| `grant.provision` | Provision one exact peer/method/target grant with generation and expiry, subject to local policy. |
| `grant.revoke` | Fence that exact grant with a new generation and cancel/deny unapplied effects under the existing revocation rules. |
| `enrollment.window` | Explicitly open a bounded hub-monotonic enrollment window or close it. Defaults closed. |
| `enrollment.invite` | Create a one-use invite bound to an expected device, window and initial exact grants. |

`build_profile` and `delegation_id` are references to trusted local provisioning,
not paths, commands, private keys or authority supplied by the caller. Source is
a pinned commit in a locally configured repository for the target; no arbitrary
repository URL, shell command or device-side compilation is accepted. The builder
never receives a coglet private key. It verifies required provenance, policy and
gate evidence before signing the exact output, as specified by the hub protocol.
An incompatible existing installation is a conflict, not an implicit uninstall,
key rotation or data reset. These operations do not bypass promotion policy.

The CLI generates an invite with at least 256 random bits, retains/displays its
secret only through an explicit protected bootstrap channel, and supplies only
its SHA-256 hash to `enrollment.invite`. The hub persists no invite plaintext.
The CLI must retain the secret securely across a lost response; retrying the
same operation cannot generate another invite. Invite creation requires the
referenced window still open and bounds expiry to that window. Restart without
a provable monotonic epoch closes windows and invalidates affected invites.
No secrets, private content or private keys belong in receipts or audit records.

### Framing, durability and recovery

Each connection carries exactly one request and one response, encoded as UTF-8
JSON, each prefixed with a four-byte unsigned big-endian byte length (1..1048576).
Reject oversize before allocation, duplicate object keys, trailing JSON values,
invalid UTF-8, unknown fields and malformed types. Bound connections and parsing
time; a malformed request is closed without admission. The schema's character
bounds do not replace this byte bound. No unsolicited events or bulk transfers.

Every operation, **including status**, is durable and idempotent. The key is
(authenticated UID, operation_id), with operation IDs using the common 32-hex
correlation-ID grammar. Atomically persist the normalized validated request
intent, its digest, audit admission and a `queued` receipt before any effect.
Digest all request fields other than `timeout_ms`; compare decoded values, not
JSON whitespace/key order. Same key and intent returns the latest retained receipt
and retained result; changed intent is `conflict`. Status retries return the
original observation, not a fresh sample; use a fresh operation ID for fresh status.
A failed operation's ID also cannot be reused. Do not evict records under pressure:
refuse new admission before effects. Retain them for the hub provisioning lifetime.

All responses carry a local operation's existing `hub-mutation-receipt` shape,
whose operation_id matches the request, or a common error for pre-admission
rejection. For an applied local receipt, `step` is the hub's durable uint64 admin-commit
sequence, not a device execution step; other states use null, exactly as the
imported receipt shape requires. Downstream receipts retain their original device
steps in the audit. Admin sequence overflow refuses new admission. `applied`
means the complete requested operation succeeded, not merely that it was queued.
Nonterminal receipts have no result. Terminal successful results are selected by
operation: status returns hub-status, window creation returns its ID, invite
creation returns its ID, and other operations return null. Failed/cancelled
operations return null and preserve their receipt/error. Repeating the original
request retrieves progress: there is no separate local receipt method.

Deploy is a durable saga, **not** an atomic transaction across hub and device.
Persist the signed artifact and deterministic, distinct child operation IDs for
push/install/start before dispatch. Preserve original downstream actor context
and exact intent. Reconcile each child's `hub.receipt` on restart, disconnect or
timeout before advancing or resending identical intent; never mint fresh IDs to
repeat uncertain effects. Recheck current authority before any new effect.
Persist child outcomes and progress in the local audit. A later-stage failure
leaves earlier applied stages recorded (for example installed but stopped); do
not claim rollback or running success. Failures before downstream admission may
be terminal; uncertain admitted children remain queued until reconciled. No next
stage on a merely queued child. Stop likewise requires a confirmed device boundary.

Configuration changes, their local receipt and audit commit in one hub storage
transaction. A status snapshot/result and its receipt commit together. Closing
the connection or expiry of `timeout_ms` only stops waiting; admitted operations
continue and retries recover them. Audit/storage failure prevents admission or
new effects; recovery must reconcile already dispatched children, not hide them.

## Alternatives

- Add `hub.deploy` or remote grant methods to 0.1: violates the closed method set
  and confuses operator configuration with peer authority.
- Have the CLI call devices directly: splits hub signing, provisioning and policy
  authority, and bypasses the hub-to-device trust model.
- Use loopback HTTP with a bearer token: adds a network listener and secret
  lifecycle where local filesystem and OS peer identity suffice.
- Edit hub database files directly: lacks a durable operation/receipt seam and
  races hub transactions.

## Consequences

This is docs/schema only: it does not implement a socket, CLI transport, builder
or deployment executor, and does not claim that deploy works today. Implementing
cards consume this ADR after merge. No existing iroh peer or device upgrade is
required merely to record the decision. Reference-hub administration requires
host service-identity access; compromise of that identity is administrative
compromise. Local grant operations remain bounded by policy, not remote requests.
A future multi-user or remote API needs a separate identity/authorization ADR.

## Verification

Schema validation: `uv run --with jsonschema --with referencing python
 docs/adr/local-operator/check_schema.py` (one shell line). The docs-only checker
validates every request operation and response alternative, imported references,
and negative controls for unknown fields/ops, actor spoofing, malformed IDs,
wrong params, receipt shape and result shape. It is not socket conformance.

Docs CI runs `bash scripts/check-assets.sh` and offline Markdown link/fragment
validation. No frozen hub schema or generated registry changes are made here.

Future implementation acceptance MUST name and execute shared real/mock tests:
`operator_peer_uid`, `operator_socket_permissions`, `operator_fail_closed_creds`,
`operator_frame_limits`, `operator_idempotency_conflict`,
`operator_status_snapshot_retry`, `operator_saga_crash_each_stage`,
`operator_receipt_audit_atomicity`, `operator_revocation_before_dispatch`,
`operator_invite_hash_only` and `operator_no_remote_listener`. These tests are
requirements, **not executed proofs** in this ADR PR. They must cover dropped
replies, restarted hub/device, partial deployment, unchanged child IDs, policy
failure and denied cross-identity receipt access.

## Approval

The orchestrator selected the local-only design; this PR records it without
reopening that decision. Acceptance requires an independent exact-head review,
green docs CI and merge of the containing PR. The PR's review and merge records
are the approval evidence; this document alone is not that evidence.
