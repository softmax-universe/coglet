# ADR-0042: Router storage on Unforged, without replacing runtime boundaries

- Status: Proposed; design only, implementation and wire qualification separate
- Date: 2026-11-26
- Authors: Coglet stage-3 planning worker, under David's staged integration plan
- Related PR: accepting PR and independent exact-head review recorded in Approval
- Supersedes / superseded by: specializes the stage-3 router-store target of
  [ADR-0040](0040-hubs-and-coglets-on-unforged.md); supersedes ADR-0031's
  router-storage restriction only for the future stage-3 profile. ADR-0031's
  current device contract remains effective until a reviewed spec revision.

## Context

The [staged plan](../cogent-design/unforged-staging.md) separates tree identity,
proof, hosted cold path, router storage and the eventual single effect path.
Stage 1 passed in [PR #214](https://github.com/softmax-universe/coglet/pull/214):
five counter steps have equal **whole filesystem tree IDs** in cogletd and
Unforged jobs, with cold misses and warm hits. It does not establish equal step
commit IDs, remote execution receipts, a crash-safe ref service or private-data
isolation. Stage 2 must pass before stage-3 implementation starts. Planning this
ADR is not permission to skip that gate.

The current source of truth remains [hub protocol](../../spec/hub-protocol.md),
[ABI](../../spec/abi.md), [promotion](../../spec/promotion.md),
[policy](../../spec/policy.md) and [tickets](../../spec/tickets.md).
[ADR-0031](0031-manage-port-and-filesystem-patches.md) made `manage.*` the
interface; its core implementation card `t_01a2104f04bd501f` is still in flight
at this inventory. Baby Bear and presence need that contract. This ADR neither
changes its acceptance nor makes it wait for an Unforged device runtime.
[ADR-0035](0035-state-hash-git-tree-id.md) already gives us SHA-256 tree identity.
[ADR-0033](0033-runtime-policy-producers.md) supplies the independently owned
applied-generation, health and emergency-output producer contract.

Unforged main inspected at `5dc0a44` already has `src/coglet_router.rs`,
`src/coglet.rs`, `crates/unforged-gitobj` and
`tests/coglet_stage3_router.rs`: ref ACLs, signed ref CAS, statement notes,
partial state packs and compaction exist. They are reuse candidates, not
qualification of this ADR. In particular they use legacy `refs/coglets` and
hub/0 vectors; ADR-0040's user model must not be inferred from those tests.

## Decision

### 1. Store objects and router decisions in an Unforged repository

Each participating router has a SHA-256 Unforged repository, with reftable
transactions. This is router-side storage, not a new Git repository inside every
near-term device. Preserve the runtime's plain `/code`, `/data`, `/logs` view,
step scheduling, effect commit boundary and grant checks. There is no new hot
path subprocess, Git binary or Unforged daemon requirement on iOS or EO1.

Use ADR-0040's user identities, not a process PID as durable authorization.
Canonical logical refs are `refs/users/<coglet-key>/{policy,published,state,inputs}`.
Here `<coglet-key>` is the enrolled 64-lowercase-hex identity component, not an
arbitrary display name or the literal `coglet:` prefix. On a router mirroring a
remote runtime these have physical prefix `refs/namespaces/<runtime-key>/`.
Runtime and user identities jointly identify an owner. A peer mirror is not the
authority to mutate its source. Signatures bind the resolved physical ref and
repository identity, so routing the same statement elsewhere cannot widen it.

- `policy`: manager-authorized candidate code, prompts and skills; never live
  activation authority. Its commit's root is the existing `/code` subtree with
  the leading `/code/` removed. No private data or logs in a policy object.
- `published`: the kernel-authorized deployment candidate after admission; a
  ref movement alone does not mean the runtime applied it or passed probation.
- `state` and `inputs`: optional, explicitly granted device exports in stage 3.
  Their availability is not assumed. Live devices retain their ABI journal.
  This ADR does not define a new device step-commit format.

All IDs in the new profile are full lowercase SHA-256 IDs. A code subtree ID,
a full ABI state tree ID and a commit ID are distinct typed values: never compare
one to another. Policy commits retain build/provenance objects and the exact
published executable identity. Their root trees must map byte-for-byte to the
current code snapshot; no rewriting or lossy projection of the ABI state hash.
Old traces, signatures and journal bytes are not silently rewritten.

### 2. `manage.patch` is a verified policy-ref push, then boundary application

On the router's new storage path, `manage.patch` means a signed CAS push to the
target's `policy` ref, not an unrestricted Git push or a new `hub.git.*` method.
The manager signs the push with current per-user policy-write authority; the
kernel independently authorizes deployment and promotion. A factory cannot
move `published`, sign as the kernel or grant itself device management rights.

The statement binds version/domain, repository, runtime/user, exact ref,
expected old commit (null only when unborn), new commit, increasing generation
and the authenticated actor/target/ticket/operation intent. CAS validation and
retention of statement, generation and original outcome are one durable
transaction. Same-ID changed intent conflicts; an identical retry returns its
original result even after newer work. Ref deletion/recreation and ABA cannot
reset the watermark. Revoked grants are not renewed by a saved receipt.

Objects arrive in bounded quarantine before any authoritative ref move. Verify
closure, SHA-256 IDs, code-only path/mode rules, size bounds, manifest/grant
admission and qualified build provenance/R3 executable. Existing 64 MiB patch
and bounded-credit transport limits remain at the device interface; a ref is
not a way to bypass them. Missing promised code/build objects fail closed.
A stale candidate has no effect on live state.

For current hub 1.0 devices, the router projects the verified code candidate
into **the existing ADR-0031 signed Patch**, retaining its base code-tree hash,
revision, generation and original receipt scope. For nonempty code, wrap the
policy root as the Git tree entry `40000 code <policy-root-OID>`: Patch.base and
Patch.revision hash the restricted snapshot with the outer `code` directory,
not the unwrapped policy commit root. For empty code, the restricted snapshot
has no entries and hashes to the empty root tree: do not create an empty `code`
directory. Derive the base from the retained prior code snapshot by the same
piecewise rule; an unborn/missing base is still null, not an empty snapshot.
The router node remains its signer. The device
has no obligation to verify a manager's ref statement or
learn Git. This adapter is a compatibility boundary, not two competing stores.
Build and retain the exact patch once, before dispatch; retry its original
bytes and operation identity rather than re-signing a different generation.

For a future ref-aware device, a separately negotiated profile may fetch policy
objects and apply the same code-tree delta. No such wire method is added here.
In either path, installed activation is serialized at the runtime step boundary;
uninstalled admission is durable verified staging consumed by install, with no
guest execution or private namespace before install (ADR-0026). Admission,
publication, applied activation and stable promotion are separate receipts.

### 3. Promotion is signed published-ref CAS, not a claim of activation

Reuse ADR-0019's reducer, retained build, approval/outbox and ADR-0033 producers.
The kernel validates candidate/base, current policy/grants, trusted gate reports,
freshness, human approval where required and the reserved generation. Persist
an immutable proposal linking those artifacts and original operation context.

The published-ref CAS compares the exact staged base and generation, retaining
the signed statement and dispatch intent atomically. The staged/published tip
must remain distinct from the device's **observed live code tree** and the
kernel's **last-known-good** record. Publication success reports `activating`,
not `stable`. Reference/mirror catch-up is not an activation acknowledgment.
Only the runtime's authenticated original-scope attestation of the exact
reserved executable/generation starts probation. Unknown outcome keeps output
fenced and is reconciled, never guessed successful or blindly republished.

Probation consumes generation-bound authenticated cumulative health on the
supervisor clock. Restart does not reset elapsed probation or counters. An
emergency output-lease revoke remains independent of router SQL/ref-store health
and the guest mutex. Rollback restores only the retained last-known-good policy
through a **new, higher-generation** signed CAS and boundary acknowledgment;
it never rewinds counters or undoes world effects/private state. A failed or
unprovable restoration fences/stops and alerts. Ref history alone is not this
state machine or an output lease.

A router SQL transaction and a ref transaction are not one transaction. Retain
a deterministic operation ID in both the promotion outbox and immutable ref
intent/outcome. Recovery first queries that original outcome: before ref commit,
retry the exact intent; after ref commit, reconcile device application with the
original operation; after application, record the same attestation. Do not mark
SQL live/stable ahead of runtime proof. Crash tests must cover every join.

### 4. Policy gates are cold-path replays

Use Unforged `stream exec` (not the historical `river exec` spelling). Gate input
pins candidate policy, base state, complete recorded inputs/replies/boundary
changes, policy revision, suite, engine/resolver and resolved worker image.
A replay executes with all relevant caches bypassed and an authenticated remote
receipt saying it executed; missing/ambiguous receipts fail the gate. A cached
fork may accelerate experimentation, but cannot masquerade as a fresh replay.
Record cache scope, execution identity, input/output IDs and verification
artifacts. The factory's report string is not trusted evidence.

Use recorded model/read outcomes, with no live model requests or adapter effects
in replay. Require explicit export/read authority for private snapshots, inputs
and replies; absent permission is an unavailable gate, not automatic upload.
Stage 1 establishes tree equality only. Device journal replay and policy checks
remain authoritative until a reviewed step-commit contract and hot/cold tests
establish stronger identity. Gate acceptance does not widen live grants.

### 5. Privacy and authority before storage convenience

Only granted objects cross runtime boundaries. A partial pack or `blob:none`
filter is not a privacy boundary: refs, commit ancestry, inputs, logs, notes and
known-OID wants can all disclose data. Enforce per-user default-deny reads over
the complete reachable object closure and on lazy promisor fetches. A code
publication grant does not grant private state or logs. If ownership/history
cannot be checked, fail closed. Retain separate `manage.read`, status and log
grants; no blanket Git read permission. Receipts contain digests, not secrets.

### 6. Reuse ADR-0031 core and disposition the held lanes

**No core acceptance change.** Keep its runtime boundary CAS, anti-replay,
original-scope receipts, patch validation, log cursor/restart behavior,
manage names/dated aliases, build-then-commit and real-iroh tests. Its internal
code-tree application seam is exactly what the router's ref adapter consumes.
The existing journal, private-data ownership and emergency fencing stay local.
Do not reinstall the removed hub/0 control plane or remove core's new manage
receiver. The router store is replaced once; the receiver is not rewritten.

All four held cards **proceed unchanged in acceptance**, subject to their
existing holds/dependencies, not an immediate release to build stage 3:

| Card | Reuse and storage boundary |
|---|---|
| `t_01a2104f2645e713` ADR-0031 sweep | Keep the manage rename and removal of obsolete `hub.git.*` uses. Router-internal Unforged object storage is not restoration of that removed wire API. Keep ordinary debug and `speech.backchannel` exclusions and dated aliases. |
| `t_01a2084063ee0c84` reference + mock conformance | Keep full hub 1.0 conformance on both backends, zero skipped acceptance cases, all-method grants and original receipt oracles. Reuse its existing driver/reverse-fetch work; do not qualify an unnegotiated ref wire profile as hub 1.0. |
| `t_01a210d847e37090` promotion producer | Keep staged-tip CAS, reserved signed generation, real runtime attestation, original receipt reconciliation and emergency passthrough. Consume one ref-store adapter/retained build, not a parallel storage implementation. Existing tests remain mandatory. |
| `t_01a20c499096d229` M3 C | Keep durable clock, probation, rollback and producer integration. Consume the existing promotion and runtime producers; a ref update does not replace health, activation or the output-lease proof. |

These are interface/semantic acceptance decisions, not certification of current
implementations. Do not reinterpret the sweep's Git-removal grep as forbidding
all router-internal object storage, or the producer's signed bundle as permission
to revive hub/0 wire. Stage-3-specific acceptance below is additional qualification
for the future backend, not a waiver of the four cards' current tests. If that
backend is unavailable, keep it gated rather than implement a second store.

## Alternatives

- **Put Unforged on every device now.** Reject: invalidates the staged plan,
  hardware constraints and in-flight core work; stage-3 router storage does not
  require per-step Git commits on devices.
- **Treat raw signed Git commits/ref ACLs as promotion.** Reject: a signature
  on content does not bind a ref transition or prove approval, activation,
  anti-replay, probation or emergency output custody.
- **Throw away ADR-0031 or rebuild the router store in each lane.** Reject:
  preserve its management/runtime boundary and adapt storage once.
- **Keep the current router backend forever.** Viable fallback if qualification
  fails; not the selected stage-3 target. Retain it during explicit cutover, not
  concurrent authoritative dual-writing.

## Consequences

This PR changes no frozen schema, protocol version, ALPN, dependency or runtime.
The future spec PR must define a negotiated router ref profile, closed signing
records/grants/errors and fixtures using common conventions; update
`spec/hub-protocol.md`, promotion/policy/tickets and generated consumers together.
Hub 1.0 stays hub 1.0; never silently replace its Patch base tree with a commit.
The dated debug-name alias window is unchanged. The full ADR-0040 device-user,
step-commit, adapter mount, attention and optimizer design is not implemented here.

At cutover, stop router mutations, import retained code trees, generations,
statements, operation receipts, proposals and last-known-good from the old store;
verify counts, digests and recovery on a disposable copy before selecting the
new authoritative backend. Preserve tombstones/watermarks and all unfinished
intents. Abort before switching on mismatch. Rollback of the backend requires
reconciling every post-cutover intent and preserving generation highwaters;
never reopen an old backup as authority or dual-write independent live stores.
This is an implementation obligation, not an executed migration proof.

### Unforged prerequisites beyond the two stage-2 gaps

The resolved-image cache key and executed-vs-cache-hit **remote** receipts remain
separate stage-2 gates; local `memo_hit` and a configured image name do not satisfy
them. Do not reopen or duplicate their existing owners. Additional source-backed
prerequisites are filed in Unforged (these issue cards are linked obligations):

1. [#9: atomic generation and original-operation outcome](https://github.com/softmax-universe/unforged/issues/9).
   `verify_and_apply_ref_statement` commits target/statement refs then best-effort
   saves `info/statement_generations.tsv`; unreadable sidecar loads empty.
   Authoritative fences/receipts must survive crash, ABA and later updates.
2. [#10: protected-ref signed context and all mutation paths](https://github.com/softmax-universe/unforged/issues/10).
   Existing RefStatement permits optional target context/legacy IDs and a routing
   namespace parameter. Require canonical repository/user/intent bindings and no
   alternate mutation route around the protected-ref gate.
3. [#11: per-user reads and private object closure](https://github.com/softmax-universe/unforged/issues/11).
   The existing state-ref parser handles `refs/coglets`, not `refs/users`;
   `authorize_upload_pack_wants` focuses on `data/` and ignores route namespace.
   Extend existing privacy primitives, do not build a second partial-pack layer.

These are source-review findings at the inspected pin, not reproduced exploits
or passing tests. Resolve/independently qualify them against the integrated pin
before stage 3; their existence in source is not acceptance. In-process object
writing, deterministic device step commits, migrations and history compaction
are later device concerns, not new router-stage prerequisites.

## Verification

This docs-only PR runs design-asset and Markdown-link checks plus required CI;
independent review checks the stage boundaries and four-card reuse mapping.

**Future, unexecuted** shared reference/mock cases and real-router/cogletd tests
must cover the following before the stage-3 backend is enabled. New wire fixtures
must validate through CDDL and JSON Schema; unit mocks alone do not qualify it:

- `router_policy_tree_projection_preserves_patch_hash`: empty, binary and nested
  code snapshots match the core's restricted-snapshot hash. Empty code uses the
  empty tree ID (also the empty policy root ID); nonempty code uses the wrapped
  tree, never the policy commit/root ID. Distinguish null/unborn bases, and
  require wrong-root negative controls for nonempty code.
- `router_policy_ref_push_preserves_manage_patch_boundary`: real iroh, valid
  code plus stale base, wrong signer, replay, oversize/path/manifest/R3 negatives;
  current core patch conflict/replay/path/oversize/tail-restart cases still pass.
- `promotion_staged_tip_cas_never_moves_live_on_failure`;
  `promotion_reconciles_original_receipt_after_crash`;
  `promotion_requires_runtime_attestation_of_reserved_generation`;
  `emergency_revoke_propagates_to_runtime` (existing producer obligations).
- `router_ref_sql_outbox_crash_reconciliation`: actual process kill before/after
  ref commit, SQL update, device boundary and acknowledgment; reopen all stores
  and recover the original intent without double apply or lost highwater.
- `router_promotion_ref_is_not_activation`: delayed/forged/stale attestation and
  disconnected observer A catching up to signed B-to-C do not falsely go live;
  equal-generation divergent and older statements are refused.
- `router_probation_ref_rollback_preserves_output_fence`: restart clock/counters,
  failed candidate and higher-generation rollback with original receipts.
- `router_cold_gate_requires_fresh_execution_and_private_export_grant`: cold/warm
  controls, missing reply, changed resolver/image, denied export and forged report.
- `router_store_cutover_preserves_pending_intents_and_generations`: disposable
  migration, crash before/after switch, mismatch refusal and fail-closed fallback.
- The named tests on Unforged #9, #10 and #11, over real transport/restart, plus
  existing `stage3_req9_*` through `stage3_req12_*` controls where applicable.

## Approval

Pending independent exact-head review, green required checks and main merge-queue
landing of the accepting ADR PR. Acceptance chooses a design; it does not certify
stage 2, stage 3, a changed wire contract or any device integration.
