# ADR-0020: Vocabulary — router, adapter, runtime

- Status: Proposed
- Date: 2026-11-23
- Authors: David Bloomin (direction); drafted by a Claude Code session
- Number: 0020. 0018 and 0019 are in use by open work; the orchestrator may
  renumber.
- Related PR: recorded in the accepting review
- Supersedes / superseded by: the "debug port, debug methods → back channel"
  row is superseded by [ADR-0031](0031-manage-port-and-filesystem-patches.md):
  the name is **manage port** (`manage.*`). Stage 1 changes prose only and does
  not change spec-v0.

## Context

The cogent design pages (`docs/cogent-design/`, proposed in PR #103) describe the system in four
words: a **cogent** is one agent made of **coglets**; a coglet runs in a
**runtime**; a runtime reaches apps and hardware through **adapters**; and the
cogent's **router** carries messages and patches between runtimes.

The specs, the code and the board use older words for two of these. "Surface"
is what the design now calls an adapter, and "hub" is what it now calls the
router. Two vocabularies for the same things make every document harder to
read, and "surface" is also a UI term in the Softmax app's design documents,
where it means a screen or panel.

The old words appear about 20,000 times in this repository, most of them in
wire names, schemas, recorded fixtures, crates and SDKs, and about fifty open
cards are building on them. Renaming everything in one change would collide
with all of that work and would change the recorded fixtures byte for byte.

## Decision

Adopt the new words everywhere, in two stages.

| Old | New | Notes |
|---|---|---|
| surface | adapter | The interface an app or a piece of hardware exposes. |
| hub, hub kernel | router | The part of a cogent that reaches runtimes. Since [ADR-0040](0040-hubs-and-coglets-on-unforged.md), "hub" in prose names a router plus an optimizer; `hub.*` identifiers still mean the router protocol. |
| device (as a protocol peer) | runtime | "Device" stays for the physical machine. |
| service call | message | A call from one coglet to another. |
| debug port, debug methods | back channel | How a coglet's manager patches it and reads its logs. |

Words that do not change: coglet, runtime, capability, manifest, inbox, event,
action, grant.

### Stage 1: words (this ADR's PR)

- The repo README, `docs/plan.md`, the runtime and factory design documents,
  and the spec pages under `spec/`, `spec/capabilities/` and `spec/messages/`
  say adapter and router in their prose.
- Identifiers do not change: method names (`hub.enroll`), id prefixes
  (`surface:`), schema and type names, file names (`hub-protocol.md`), crate
  and SDK names, fixtures. Anything in backticks or a code block is untouched.
- Each changed spec page carries a one-line note saying so.
- ADRs and review records are history and are not rewritten. Neither are the
  two Softmax app design documents, where "Hub" is literal UI copy and
  "surface" means a screen or panel.
- Unquoted field names in prose, such as the `(surface, item)` key, keep the
  old word, because they are identifiers.
- The runtime already had a "router" of its own, the part that orders and
  delivers events. It is now always called the **event router**, so that
  "router" alone means the cogent's router. Four sentences in the spec prose
  change for this, all plainly about the runtime; none changes a rule. A fifth,
  "The router MUST enforce the target's event grants and capability ownership
  before relay" in `spec/hub-protocol.md`, is left word for word as it was:
  whether it binds the runtime or the hub is ambiguous on main and is for the
  hub-protocol owner to settle, not this ADR.
- New prose, new cards and new documents use the new words from now on.

Stage 1 changes no schema, no fixture and no behaviour, so it is not a change
to a frozen contract.

### Stage 2: identifiers (separate cards, each with its own ADR if it touches a frozen contract)

Renamed one area at a time, in this order, so that no area is renamed under a
consumer that has not moved:

1. Spec schemas, CDDL, method names and id prefixes, with their fixtures
   regenerated. This is a wire change and needs a protocol version.
2. Generated types and the code generator.
3. Crates (`coglet-hub`) and the runtime's internals.
4. SDKs and the `surfaces/` directory.
5. The conformance harness and its cases.
6. Product repositories that pin released artifacts.

Until an area's card lands, its identifiers keep the old words and mean the
same things.

## Alternatives

- **Words only, for good.** Cheap, but the code would keep the old words
  permanently and every new reader would have to learn both.
- **Everything at once.** The cleanest end state, but it conflicts with every
  open pull request and running card and rewrites every recorded fixture in one
  step.
- **Keep the old words and change the design pages instead.** "Surface" already
  means something else in the app's design, and "hub" hides that the router is
  itself a coglet.

## Consequences

- For a while, prose and identifiers use different words. The note on each spec
  page says which is which.
- "Adapter" was already used loosely in a few places for other things, for
  example "deployment adapter". Those uses should be reworded as they are
  touched.
- Stage 2's first step is a breaking wire change and must be scheduled, not
  slipped in.
- Open cards are not rewritten. They are told about this ADR and use the new
  words in anything new they write.

## Verification

Stage 1 is verified by the existing checks staying green with no schema,
fixture or code change in the diff: the spec tests, the conformance suites and
the offline link check.

## Approval

Not yet reviewed for acceptance.
