# ADR-0036: A cogent has no centre; it is present on a few coglets

- Status: Proposed
- Date: 2026-11-25
- Authors: David Bloomin (design), David's Claude Code session
- Related PR: the publishing PR
- Supersedes / superseded by: none. Amends the [cogent design](../cogent-design/README.md)
  pages, which described "the cogent's centre". Builds on the router of
  [ADR-0020](0020-vocabulary.md) and the manage port of
  [ADR-0031](0031-manage-port-and-filesystem-patches.md).

## Context

The cogent design pages said a cogent runs "a few coglets always running at its
centre" (fast loop, factory, optimizer, memory, router) with "devices around
that centre". That reads as if a cogent lives in one place, usually Softmax
infrastructure, and the coglets on devices are its limbs.

That is not the intended system. Every coglet runs on some runtime; a runtime on
Softmax infrastructure is a runtime like any other. Memory, a database, the
factory and the fast loop are coglets in the same sense as Face, a controller or
the Blender coglet. What holds a cogent together is its identity and its router.

What the design also lacked is a way to say where the cogent's attention is.
A cogent may have dozens of coglets running. It cannot watch all of them at the
speed it watches the conversation the user is having right now, and it should
not try: most coglets should simply get on with their job.

## Decision

### 1. A cogent is distributed

A cogent is an **identity**, a **name**, an **owner**, a set of **coglets** on any
number of runtimes, and one **router** that connects them.

- No coglet and no runtime is the centre. The fast loop, factory, optimizer,
  memory and database coglets run wherever the cogent places them, often on a
  Softmax-hosted runtime, but that runtime has no special role.
- Coglets reach each other only through the router, or through their own
  runtime when both are on it. Nothing else changes about routing.
- The router is itself a coglet, as the design pages already say.

### 2. Presence

The cogent is **present** on a subset of its coglets. Presence is **attention**:
the cogent is paying attention to those coglets now. Every other coglet runs
autonomously, on its own policy and memory, with occasional management and
check-ins.

| | Present coglets | Absent coglets |
|---|---|---|
| What they do | Their normal work | Their normal work |
| What they send to the cogent | Delivered to its attention as it happens, ahead of other traffic | Held and delivered in check-ins |
| How fast the cogent answers | Within the coglet's latency budget | At the next check-in, or when the coglet asks for attention |
| Who speaks for the cogent | Present coglets, such as Face | Nobody, unless asked |
| Management | As needed | Occasional: status, logs, patches through the manage port |

The cogent's **attention** is the fast loop coglet: it receives what present
coglets send, decides what to do, and sets presence. It runs on some runtime like
every other coglet.

Typical presence: Face on the device the user is using, and the app coglet for
the app they are working in. The factory, memory, database and game coglets are
usually absent, working through their queues.

Presence does **not** change grants, start or stop coglets, or change what a
coglet may do. Coglet-to-coglet messages are unaffected; presence governs only
traffic addressed to the cogent's attention.

### 3. Presence API

The router holds the cogent's presence and offers these messages.

```idl
Presence
  cogent    CogentId
  present   [CogletId]   The coglets the cogent is paying attention to.
  limit     u8           At most this many are present at once. Default 4.
  revision  u64          Increases on every change.

PresenceChange
  base      u64          The revision this change was made from. Refused if presence has moved.
  add       [CogletId]
  remove    [CogletId]
  reason    str?

PresenceRequest
  # An absent coglet asking for attention.
  coglet    CogletId     Filled in by its runtime, never by the coglet.
  urgency   "low" | "high"
  reason    str

CheckIn
  # What the attention receives about one absent coglet.
  coglet    CogletId
  status    CogletStatus  From the manage port's status.
  held      [Message]     What it addressed to the cogent since the last check-in, oldest first.
  dropped   u64           Messages dropped once the hold limit was reached.

Router
  presence.get() -> Presence
  presence.set(PresenceChange) -> Presence
      Only the attention coglet and the owner may change presence.
  presence.request(PresenceRequest)
      Any coglet may ask. Delivered to the attention at once; it decides.
  presence.changed(Presence)
      An event to subscribers, and to every coglet that entered or left presence,
      so a coglet knows whether it is being attended.
  presence.check_in(CheckIn)
      Sent to the attention for each absent coglet at the cadence it set, or sooner
      when the coglet's hold limit is reached.
```

Rules:

- `presence.set` is a compare-and-swap on `revision`, so two writers cannot undo
  each other silently.
- Adding past `limit` is refused; the caller removes something first.
- A coglet that stops or is removed leaves presence, and `presence.changed`
  says so.
- Held messages are bounded per coglet. Past the bound, the oldest are dropped
  and counted in `dropped`; a `high` `presence.request` is never dropped.
- The attention sets each absent coglet's check-in cadence. The default is a
  few minutes, and a coglet with nothing held sends a status-only check-in at a
  slower rate.

### 4. Spec changes

`spec/hub-protocol.md` adds the `presence.*` messages above as an additive
change, in the next minor version of the router protocol, with CDDL in
`spec/messages/` and cases in `conformance/hub/`. Nothing existing changes
meaning. The spec text lands with the implementation card, not in this ADR's PR.

The design pages drop "the cogent's centre": the README, the cognitive
architecture, app integration, and the unforged, LLM and secrets pages say
"a Softmax-hosted runtime" where they meant a place.

## Alternatives

- **Keep a centre.** Simpler to draw, but it ties the cogent's identity to one
  set of runtimes and makes Softmax infrastructure special in a way the protocol
  does not need.
- **Presence as where the user is.** Narrower: the cogent may attend to a coglet
  the user is not looking at, such as a game it is playing. Presence is about
  attention, not location.
- **Attend to everything equally.** Does not scale in cost or latency, and gives
  no way to let coglets work quietly.
- **Presence as a grant.** Mixing attention with authority would mean losing
  attention also loses permissions. Kept separate.

## Consequences

- The cogent design pages and their diagrams are rewritten without a centre, and
  explain presence.
- The fast loop's role is named: it is the cogent's attention.
- Model spend and latency budgets concentrate on present coglets. Absent coglets
  need policies that can run without the cogent answering at once.
- The router protocol grows one additive message family. Existing traffic and
  spec-v0 fixtures do not change.
- Only the attention and the owner can change presence, so a coglet cannot pull
  the cogent's attention to itself except by asking.

## Verification

Named conformance cases, against both the reference router and the mock:

- `presence_set_requires_attention_or_owner`
- `presence_set_is_cas_on_revision`
- `presence_limit_refuses_extra`
- `presence_present_delivers_immediately`
- `presence_absent_holds_until_check_in`
- `presence_request_reaches_attention`
- `presence_high_request_never_dropped`
- `presence_changed_reaches_entering_and_leaving_coglets`
- `presence_changes_no_grants`

## Approval

Pending independent review and the accepting PR.
