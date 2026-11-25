# ADR-0031: Manage port, and patches on a plain filesystem

- Status: Accepted decision (PR #157); hub 1.0 wire implementation pending independent review and merge
- Date: 2026-11-25
- Authors: Coglet Orchestrator (on David's instruction)
- Related PR: [#157](https://github.com/softmax-universe/coglet/pull/157) (decision); accepting spec PR linked in Approval
- Supersedes / superseded by: supersedes the "debug port, debug methods → back
  channel" row of [ADR-0020](0020-vocabulary.md); replaces the
  [Git control plane](../../spec/hub-protocol.md#filesystem-patches-and-log-tails) of
  [ADR-0004](0004-hub-protocol.md) in the hub 1.0 wire revision below.

## Context

David, 2026-11-25:

> The Coglet design had a Git repo built into it. I think we can simplify it
> and just have a file system for now. Coglets should process events and
> accept patches and allow tailing logs for the manager. Patch does not need
> to run on Git initially. We can add that later as a Gitlet.

> Coglets should have a manage port rather than a debug port. Let's just call
> it manage everywhere.

Today a coglet's manager reaches it in two ways:

- the `debug.*` methods (`debug.read`, `debug.write`, `debug.pause`,
  `debug.step`), which [ADR-0020](0020-vocabulary.md) renamed in prose to the
  "back channel";
- the Git control plane: one Git repository per coglet, three signed refs
  (`refs/heads/hub`, `refs/published/main`, `refs/heads/logs`), Git bundle
  transfer (`hub.git.push`, `hub.git.fetch`, `hub.git.refs`), `git fsck`
  quarantine and a signed `git archive` digest.

The design pages ([Coglets and runtime](../cogent-design/coglets-and-runtime.md))
already describe what a manager needs in Git-free terms: `patch`, `tail_log`,
`read_state`, `status`, `pause` and `step`. Git adds a second object model
(SHA-1 object ids, bundles, refs, fsck limits) that no coglet uses for its own
work. The runtime already stores `/code`, `/data` and `/logs` in a
copy-on-write, content-addressed filesystem (`coglet-fs`) with tree hashes.

## Decision

### 1. One name: manage

The way a coglet's manager changes, inspects and controls it is the **manage
port**. "Debug port", "debug methods" and "back channel" are retired
everywhere: prose, specs, method names, grants, types, crates, SDKs, CLI,
tests and the visualizer.

| Old | New |
|---|---|
| debug port, back channel | manage port |
| `debug.read` | `manage.read` |
| `debug.write` | `manage.write` |
| `debug.pause` | `manage.pause` |
| `debug.step` | `manage.step` |
| `runtime.debug_step` (event) | `runtime.manage_step` |
| `hub.git.push` (code) | `manage.patch` |
| `hub.git.fetch` of `refs/heads/logs` | `manage.tail_log` |
| `hub.git.fetch` of `refs/published/main`, `hub.git.refs` | removed in 0.1 (a later Gitlet) |
| debug agent (runtime), debug client | manage agent, manage client |

Each `manage.*` method keeps its own grant, as each `debug.*` method does
today. Reads stay off unless granted. "Debug" remains an ordinary English word
for debugging; it is no longer the name of an interface.

### 2. A coglet is a plain filesystem

A coglet is stored as files: `/code` (its policy: `bin/step.wasm`, prompts,
skills, manifest, build record), `/data` (private state, including the inbox)
and `/logs` (append-only). There is no per-coglet Git repository, no refs and
no Git object ids. Tree identity is the existing `coglet-fs` tree hash
([spec/abi.md](../../spec/abi.md) "state hash" definition).

### 3. `manage.patch` replaces Git push

A patch is a signed change to `/code`:

```idl
Patch
  base       TreeHash?     The /code tree it was made from; absent only for install.
                           Refused with conflict if the coglet has moved on.
  revision   TreeHash      The /code tree after applying it.
  generation u64           Strictly higher than the last applied patch (anti-replay).
  files      [FileChange]  Whole-file puts and deletes under /code. Never /data or /logs.
  signature  bytes         Router node key over the canonical CBOR of the fields above.

FileChange
  path   str     Canonical relative path under /code.
  bytes  bytes?  New content; absent means delete.
```

What carries over unchanged from the Git control plane, minus Git:

- the runtime verifies signer, `base`, `generation` and the resulting
  `revision` in one durable compare-and-swap. For an installed target it applies
  the patch only at the serialized runtime step boundary. For an uninstalled
  target, [ADR-0026](0026-router-push-applied-boundary.md)'s pre-install boundary
  carries over as durable verified staging consumed by install: no guest code,
  no private namespace and no live revision until install. Admission is not
  completion; retain the original operation's scope and durable receipt;
- path rules: no symlinks, absolute paths, traversal, or anything outside
  `/code`; size limits stay (64 MiB per patch);
- a code patch must leave a valid `bin/step.wasm`;
- build-then-commit stays a router responsibility, including the
  [ADR-0003](0003-determinism-rules.md) rewrite and the `build/provenance.cbor`
  record, whose `source_commit` becomes `source_tree`;
- rollback is a new, higher-generation patch that restores a last-known-good
  tree; there is no rewind;
- `status` reports the live `/code` tree hash and generation.

### 4. `manage.tail_log` replaces the logs ref

`manage.tail_log(coglet, from_offset)` streams `/logs` entries as they are
written, from a byte offset, with the same grant and size limits as today's
log reads. Logs are no longer batch-committed anywhere.

### 5. Git later, as a Gitlet

Version history, publishing a coglet's code for others to fetch, and syncing
with outside repositories become a separate **Gitlet**: a coglet or adapter
that turns Git operations into `manage.patch` calls and log reads. It is not
part of 0.1 and needs its own ADR.

### 6. How it lands

- This ADR and the design pages (`docs/cogent-design/`, `docs/plan.md`)
  change now, in prose only.
- The spec change is one breaking wire change against the
  [ADR-0009](0009-spec-v0-freeze.md) freeze: protocol version bump, fixtures
  regenerated, and a compatibility window in which peers accept both the
  `debug.*` and `manage.*` names. `hub.git.*` has no alias; it is removed.
- Hub message version is **1.0**; the framing ALPN `coglet/hub/0` stays unchanged
  (its suffix identifies transport framing). Common/surface 0.1 schemas and
  ADR-0020 stage-2 hub/surface identifiers are unchanged; there is no downgrade
  to the old hub method set.
- Accept `debug.read`, `debug.write`, `debug.pause`, `debug.step` as name-only
  input aliases until **2027-02-25T00:00:00Z** (exclusive). Normalize to the
  corresponding manage method before schema validation, grant lookup, intent
  hashing, receipt and audit storage; emitted names are canonical. Verify signed
  delegation bytes before canonicalizing its method grants. Historical
  `runtime.debug_step` is mapped only by explicit archive import into a new
  trace, never in-place mutation of journal bytes. See the normative
  [alias table](../../spec/hub-protocol.md#compatibility-alias-table-temporary).
- `Patch.bytes` and its signature are lowercase hex on the common profile wire,
  not CBOR byte strings. The 64 MiB cap counts the complete deterministic-CBOR
  Patch, including hex overhead and signature. A `manage.patch` request declares
  its size/hash and uses bounded credit/chunk/end frames. This keeps the 1 MiB
  frame cap; it does not add binary values to the common CBOR profile.
- A tree hash uses the existing ABI path-to-hex snapshot hash restricted to
  absolute `/code/...` paths. Status returns `live_tree` and `generation` (both
  null pre-install); install consumes the exact applied staged revision.
  Sorted unique FileChanges use 1..32 relative ASCII path segments (1..128 bytes
  each), 1..4096 entries, whole-file puts/deletes and no symlink traversal.
- Patch signatures bind the pinned router and coglet target plus every Patch
  field except signature in the `coglet/hub/0:manage.patch\n` signing domain.
  Base/generation CAS, original-scope receipts and ADR-0026's two apply
  boundaries survive restart; pre-install staging never creates live state.
- `manage.tail_log` uses a durable per-coglet ordered canonical-JSONL stream of
  `/logs` append entries (`path`, `file_offset`, `hex`). Byte offsets survive
  restart and file rotation; each credit-bounded operation streams at most
  65536 bytes and returns `next_offset`. No implicit export/read grant is added.
- The spec-only migration regenerates new ABI timeline companions under
  `spec/fixtures/abi/*-timeline.fixture`. Legacy 0.1 implementation harness
  inputs under `conformance/` remain unchanged for the separately owned runtime
  conversion. Passing new specification oracles does not qualify those runtimes.
- Work in flight (the ADR-0018 deploy pipeline, applying router pushes, the
  runtime receiver, H-transform publication) lands as it is and is converted
  afterwards, so no reviewed work is thrown away.
- The [ADR-0020](0020-vocabulary.md) stage-2 renames (hub → router,
  surface → adapter) are unchanged and do not repeat this rename.

## Alternatives

- **Keep Git and only rename.** Keeps a second object model, bundle transfer
  and fsck limits that no coglet needs for its work.
- **Keep "back channel".** David chose "manage".
- **Rename now, simplify later.** Two breaking wire changes instead of one.

## Consequences

- One breaking hub protocol change, version 1.0, with the dated alias window above.
- The runtime, router, CLI (`coglet push`), mocks and conformance lose their
  Git dependencies; `git fsck`/bundle limits and SHA-1 ids disappear from the
  wire.
- Lost in 0.1: fetching a coglet's published code and its history. A router
  keeps its own audit of every applied patch; a Gitlet can add history later.

## Verification

- Spec: `spec/tests` pass on regenerated `manage.*` fixtures, including
  patch conflict (stale `base`), replay (non-increasing `generation`), path
  rejection, oversize, missing `bin/step.wasm`, and a tail across a restart.
- Conformance: the router suite passes against both the reference router and
  `coglet-mock`, and old `debug.*` names are accepted only during the window.
- Audit `git grep -n -i "debug\.\(read\|write\|pause\|step\)\|hub\.git\|back.channel"`
  after the rename: no management-interface uses remain outside ADR history.
  `speech.backchannel` is an unrelated conversational acknowledgement and stays
  unchanged; its matches are not management-interface rename failures.

## Approval

Direction: David, 2026-11-25 (quoted above). The docs-only decision landed in
[PR #157](https://github.com/softmax-universe/coglet/pull/157), merge commit
`8629abd49132dd97bad8472143f03e0c2ae10723`, after
[independent exact-head approval](https://github.com/softmax-universe/coglet/pull/157#pullrequestreview-6050166767).

The hub 1.0 spec PR must have green required checks and independent exact-head
approval before the main merge queue lands it. That review/landing accepts the
wire revision, not runtime/router/CLI/mock conformance or ADR-0003 qualification.
Spec PR: PENDING_SPEC_PR.
