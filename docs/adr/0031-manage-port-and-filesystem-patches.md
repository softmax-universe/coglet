# ADR-0031: Manage port, and patches on a plain filesystem

- Status: Proposed; direction decided by David 2026-11-25, wire change not yet scheduled
- Date: 2026-11-25
- Authors: Coglet Orchestrator (on David's instruction)
- Related PR: this ADR's PR
- Supersedes / superseded by: supersedes the "debug port, debug methods → back
  channel" row of [ADR-0020](0020-vocabulary.md); replaces the
  [Git control plane](../../spec/hub-protocol.md#git-control-plane) of
  [ADR-0004](0004-hub-protocol.md) once its wire change lands.

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
  `revision` in one durable compare-and-swap, and applies the patch only at
  the step boundary of [ADR-0026](0026-router-push-applied-boundary.md);
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

- One breaking protocol change, to be scheduled with a compatibility window.
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
- `git grep -n -i "debug\.\(read\|write\|pause\|step\)\|hub\.git\|back.channel"`
  finds nothing outside ADR history once the rename completes.

## Approval

Direction: David, 2026-11-25 (quoted above). Independent review and the
accepting PR are recorded here when they happen.
