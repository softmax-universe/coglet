# ADR-0016: Recorded game observations and motor-only game extensions

- Status: Proposed
- Date: 2026-11-24
- Authors: Coglet contributors
- Related task: `t_01a2084a93a874de`
- Related PR: recorded in the accepting review
- Supersedes / superseded by: none; extends the trusted extension catalog mechanism, not spec-v0

## Context

[Operation bindings](../../spec/capabilities/bindings.md) allow release-pinned
extension catalogs but explicitly forbid inventing a guest event from a surface
read reply. The [surface protocol](../../spec/surface-protocol.md) has no recipient
field on events. A game read may contain private viewer-scoped state. Granting
all readers a common event name cannot make cross-owner broadcast safe.

Mindister's [device mapping](https://github.com/softmax-universe/mindister/blob/a7b3212/docs/components/game-devices-from-companion.md)
selects the `/dev/game` aliases, units and limits. Its merged device libraries
are owner-side projections, not production surface implementations. In particular,
context fixtures do not establish native resource aggregation or objective
semantics; research has not landed its owner projection. The frozen Mindister
intent enum excludes research. Neither envelope validation nor renaming research
as `configure` or `open_panel` authorizes its native effect.

## Decision

Approve the following **product extension design**, conditional on independent
review and merge of this ADR. Implementation follows in Mindister, with the
product-neutral Java SDK in Coglet. This document changes no frozen file,
message discriminator, ABI import, built-in operation or legacy intent enum.
Merging it approves the design, not production enablement or qualification.

### Closed, pinned catalog

Mindister owns a versioned `game/1` catalog and bundled closed input, successful
output and event schemas. Deployment pins its exact content digest, the adapter
build, the Coworld source/patch/artifact manifest and the Coglet SDK/release.
Schema URIs resolve to these local trusted bytes; no dispatch-time network fetch,
mutable remote schema or generic profile-map fallback is permitted. Unknown
fields, unsupported operations and valid sibling-operation shapes are rejected.
The implementation PR must include machine-readable schemas, positive and
negative fixtures, and exact operation-to-rule selection tests.

The mapping is the inventory, not permission to advertise it all. The closed
candidate inventory is:

| Direction | Selected operations |
|---|---|
| Read | `game.state`, `game.tiles`, `game.blocks`, `game.resources`, `game.objectives`, `game.find`, `game.highlights`, `game.logistics`, `game.tech`, `game.helper`, `game.navigation`, `game.destinations`, `game.recording`, `game.history` |
| Presentation action | `game.highlight`, `game.clear_highlights` |
| World/control action | `game.move`, `game.mine`, `game.build`, `game.stop`, `game.research`, `game.helper_spawn`, `game.helper_stop`, `game.navigate`, `game.play`, `game.continue`, `game.prepare_quit` |
| Event | `game.observation`, `game.recorded` |

Existing legacy intents (`place`, `rotate`, `configure`, `deconstruct`,
`set_processor_code`, `open_panel`) are not new surface operation names or aliases.
Any later addition needs a new ADR. Debug, engine edits, Java reload and generic
native invocation are absent. An operation appears in exactly one capability
entry and has both directions bound to its exact schema. An unmerged producer
or unqualified native device is unavailable and unadvertised; optional absence
must not block the other groups or be represented by fabricated data. A qualified
operation may return a schema-defined unavailable view or common `unavailable`
error for an unsupported current mode. Absence never falls back to omniscient
state, direct native calls or a legacy wire.

### Recorded observations, not guest reads

The surface is bound to **one permitted coglet identity** for each game view.
Its source identity, immutable viewer/team binding, local ledger and event queue
belong to that principal. The supervisor must reject multi-coglet bindings on
this adapter. A distinct owner uses a distinct surface identity/link/store; no
private event is sent over a shared multi-owner surface. Runtime routing must
honor that binding and the owner's event grant before materialization. A runtime
that cannot enforce it is incompatible and gets no production game registration.
Neither `game.observation` nor `game.recorded` permits cross-owner fanout.

A trusted local supervisor configures at most 16 observation queries per view.
Queries select only catalog reads and their exact validated argument shapes;
query identity is a fresh 32-lowercase-hex ID bound to operation, canonical args,
viewer, world generation, catalog digest and grant epoch. Reconfiguration creates
new IDs. No guest-controlled path or synchronous ABI read is introduced. A
supervisor may issue ordinary runtime reads separately, but a `read_reply` is
never automatically converted to an event.

The native owner samples those configured queries passively on its normal update,
without stepping, unpausing, saving or mutating the simulation. Each delivered
snapshot is an ordinary recorded `game.observation` event. Its payload is the
closed record:

```
{query_id, operation, args, revision, world_generation, grant_epoch, value}
```

`query_id` is the bound query ID. `operation` is one of the read names above;
`args` and `value` must select that operation's closed input/output rules.
`revision`, `world_generation` and `grant_epoch` are uint64. The owner allocates
revision monotonically within its retained surface history, including an empty
or unavailable snapshot; native tick is not revision or surface logical time.
World replacement invalidates pending snapshots and cursors, not source sequence
continuity. A paged value includes the device's `items`, `next_cursor`,
`has_more`, `truncated` and `coverage`; do not replace an empty continuation with
end-of-query. The entire encoded payload, including args and wrapper, is bounded
to 262,144 bytes. Over-budget data yields a bounded refusal, not silent omission.

To publish an observation, require the same owner to hold both `game.observation`
and the selected read operation, with current epoch and unexpired deadline/lease
at sampling and again after bounded encoding. A cursor binds world, viewer,
query, catalog and epoch. Fencing discards pending private snapshots; callbacks
capture the original generation and never consult a replacement binding. The
runtime journals the exact event before a guest step. Replay consumes recorded
bytes only, never samples live state or continues a live cursor. Observation
configuration is local trusted provisioning, not a new surface wire message,
Hub method, grant source or implied guest authority.

### Game actions and research

All world/control actions traverse ordinary Hub motor arbitration. The product
motor gains an explicit closed `game/1` extension dispatch table, distinct from
the frozen legacy intent schema. Admission validates selected operation/args,
request attribution, policy and operation/resource grants; the ordinary relay
retains that typed operation and validates it again on the native owner. There
is no direct coglet-to-engine path and no permissive generic native-command hook.
If the motor does not support a selected extension, it remains unavailable.
Changing a frozen legacy schema would require its own approved versioned recut;
this ADR does not authorize editing it.

`game.research` arguments are the closed `{content}` record: exact unambiguous
pinned-engine content name, nonempty valid UTF-8, at most 200 encoded bytes.
A successful result is the closed record
`{content, spent, paid, remaining, unlocked}`. Resource arrays contain at most
100 unique item rows `{item, amount}` each; `item` is an exact content name with
the same byte bound, and `amount` is a nonnegative uint64. `spent` is the delta of
this request; `paid` and `remaining` are the native post-request totals for that
node; `unlocked` is native unlock state, not `unlockedNow` in noncampaign mode.
Reject a node whose complete cost vectors cannot fit these bounds; do not page
an atomic spend result or present incomplete totals as complete. Common failed
outcomes have an error, not a successful result with guessed zero spend.

Require `game.research` operation and `game.control` exclusive resource grants,
authenticated owner/request, current epoch, native world/viewer generation and
exclusive deadline. Reject noncampaign, custom-pool and multiplayer execution.
On the native owner, check **every ancestor**, every required objective, campaign
eligibility and the permitted native resource pool atomically with the actual
`CampaignResearch.research` call. Preserve partial funding and native spending;
never set unlocks, inject resources or infer success from queue admission.
If effect completion crosses the deadline, report expired/uncertain as applicable
and preserve actual late spending as audit, never a fabricated rollback/success.

Stop/revoke/kill/generation replacement serialize with that bounded native effect
under the same authority fence. Stop does not wait on disk, telemetry or the
control queue. Persist the full mapped act bytes before acknowledgement and
before dispatch; exact retained `(coglet,id)` retries return historical receipts
before current-epoch admission. Changed bytes or epoch conflict. Retain receipts
for at least 24 hours after the later of deadline and terminal time. An uncertain
native effect is never replayed, including after crash or reconnect. This is
attribution and deduplication, not exactly-once physical execution.

### Recorder attribution and native producers

Persist request -> Coglet action -> relay plan -> native result through the Hub
motor **before act admission**. Joins live in the owner's authoritative ledger;
no new `plan_id` envelope field. Rejected request logging refuses new automated
dispatch. Human and ambient events have no inherited agent request identity.
Semantic `game.recorded` telemetry never mints `action.outcome`, and its loss
cannot evict authoritative terminal receipts. Use the existing recorder library,
its selected private fields and digest-only provenance, not a second recorder.

Native semantic listeners, inventory/shared-core separate pools, campaign
eligibility, objective predicates/counters/timers/waves, block availability,
logistics update hooks and helper ownership need explicit native integration
proof. Navigation in an unsupported headless mode remains unavailable. Fixed-point
thousandths and integer milliseconds are quantized at the native projection;
no Java floats, arbitrary paths, fog-free verification data or diagnostic text
cross into guest observations. The selected 100-row page, per-device scan bounds,
1024-entry/8-MiB semantic queue and 128-frame wire queue limits remain in force.

## Alternatives

- Implicit read-reply-to-event conversion: rejected; unspecified guest contract,
  no selected operation binding, and unsafe multi-owner delivery.
- New synchronous ABI import, `runtime.*` event or SP/1 tunnel: rejected; unnecessary
  frozen-boundary changes and bypass of journals, identity or grant semantics.
- Adding research to `configure`/`open_panel` or direct local native dispatch:
  rejected; hides authority and bypasses motor fencing and native spend receipts.
- Advertising fixture-backed devices as native production capabilities: rejected;
  fixture conformance is not evidence of native producers or owner aggregation.
- Waiting for all M1 work before any adapter work: unnecessary. Build against
  merged main and released `v0.0.0-rc1` cogletd. If a specific final proof needs
  M1's proven path, identify and wait only for that proof; do not waive it.

## Consequences

This adds no built-in game capability or default grant. Existing frozen peers
remain unchanged; deployment explicitly opts into trusted game/1 schemas and
single-owner binding. Device groups can qualify incrementally. Private per-owner
links cost extra stores/connections, but avoid an unversioned recipient-field
change. Approval does not authorize debug, hot reload, legacy compatibility or
native execution without the ordinary Hub motor. Rollback disables game catalog
registration and fences active work; it cannot undo an already-paid research effect.

## Verification

The implementation must publish and run these named tests (names are the
acceptance contract, not a claim they exist or pass yet):

- `GameObservationContractTest`: exact selected args/value schemas, unknown fields,
  sibling-shape rejection, complete payload-byte cap, unavailable/empty continuation,
  bounded query count and no implicit read-reply conversion.
- `GameObservationIsolationTest`: reject multi-coglet bindings; wrong source/owner,
  lost event/read grant, revoke/re-enable ABA, changed generation/query/catalog,
  exclusive expiry during encoding, and stale queued callback all disclose no data.
- `GameObservationReplayTest`: journal and replay identical selected event bytes
  without live reads or simulation ticks; include gaps, pages and world replacement.
- `GameResearchNativeTest`: on the exact pinned Coworld build, prove real partial
  spend/unlock accounting, a locked grandparent, unmet objective, separate resource
  pools, disallowed mode and read-only tech; no direct-unlock substitute.
- `GameResearchMotorRaceTest`: exercise the actual Hub motor/relay/native path,
  with revoke, kill, generation and deadline crossings before native effect and
  before terminal publication, including a successful control.
- `GameResearchRetryTest`: identical pending/terminal retries spend once; changed
  envelopes conflict; restart after uncertain dispatch never resends the effect.
- `GameRecorderMotorAttributionTest`: ordinary Hub request/action/relay/native joins,
  request-log failure refusal, human isolation, native listeners and terminal-outcome
  preservation under lossy semantic queue pressure.
- `GameNativeDeviceProjectionTest`: carry the device cards' named Companion-case
  mappings into real native aggregation/predicates/timers and immutable owner snapshots;
  disclose unsupported modes rather than treating fixture assertion ports as parity.
- `GameSurfaceRecordedFixtureTest` plus the shared language-neutral **surface**
  conformance suite through the actual Java adapter, deterministic CBOR and private
  AF_UNIX stream with fragmented frames, wrong-direction and fatal-disconnect cases.

Required CI, independent exact-head implementation review and merged product/SDK
PRs are mandatory. A recorded adapter fixture is allowed for the general adapter
gate, but does not replace explicitly required native research/motor/producer proof.
Native engine source, patch and artifact identities must be recorded separately
from fixture results. No green generic schema suite qualifies the private native
path, runtime routing or physical durability.

## Approval

Proposed only. Independent review and the accepting PR must be recorded before
implementing this contract. After acceptance, production guest registration still
requires all applicable implementation gates above; unqualified operations stay
disabled. No implementation or qualification is claimed by this ADR.
