# ADR-0036: A cogent has no centre; it is present on a few coglets

- Status: Proposed
- Date: 2026-11-25
- Authors: David Bloomin (design), David's Claude Code session
- Related PR: #182
- Supersedes / superseded by: superseded in part by
  [ADR-0040](0040-hubs-and-coglets-on-unforged.md) (the router is the attention;
  one attending router among many). Amends the [cogent design](../cogent-design/README.md)
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
- Coglets reach each other through the router, or through their own runtime
  when both are on it. Nothing else changes about routing.
- The router is itself a coglet, as the design pages already say.

### 2. Presence

The cogent is **present** on a subset of its coglets. Presence is **attention**:
the cogent is paying attention to those coglets now. Every other coglet runs
autonomously, on its own policy and memory, with occasional management and
check-ins.

The cogent's **attention** is one named coglet, normally the fast loop. It
receives what present coglets send, decides what to do, and sets presence. It
runs on some runtime like every other coglet.

| | Present coglets | Absent coglets |
|---|---|---|
| What they do | Their normal work | Their normal work |
| What they send the attention on their own initiative | Delivered as it happens, ahead of other traffic | Held, and delivered in check-ins |
| Answers to calls the attention made | Delivered as they arrive | Delivered as they arrive |
| How fast the cogent responds | Within the coglet's latency budget | At the next check-in, or when the coglet asks for attention |
| Who speaks for the cogent | Present coglets, such as Face | Nobody, unless asked |
| Management | As needed | Occasional: status, logs, patches through the manage port |

**What is held.** Only messages an absent coglet sends to the attention on its
own initiative: reports, requests for help, feature requests. A message that
carries the `correlation_id` of a call the attention made is never held. That
covers the answer itself (`memory.result`, `ticket.result`) and the updates that
follow from the call (`ticket.update`), so the attention's own work never waits
for a check-in. Messages between other coglets are unaffected. The attention
coglet itself is always treated as present; it is not listed in `present` and
does not count toward `limit`.

**Who holds them.** The router. It holds what crosses runtimes. A runtime that
would deliver locally, because the sender and the attention share it, forwards
such a message to the router instead, so every hold, bound and check-in is the
router's. The router tells each runtime the cogent's presence along with its
other router-to-runtime traffic, so a runtime knows which messages to forward.

Typical presence: Face on the device the user is using, and the app coglet for
the app they are working in. The factory, memory, database and game coglets are
usually absent, working through their queues.

Presence does **not** change grants, start or stop coglets, or change what a
coglet may do.

### 3. Presence API

The router holds the cogent's presence and offers these messages. `CogentId`,
`Presence`, `PresenceChange`, `PresenceRequest` and `CheckIn` are new types;
`CogletId` is the existing coglet id, and `CogletStatus` is the result of the
manage port's `status`.

```idl
Presence
  cogent     CogentId
  attention  CogletId      The coglet that receives what present coglets send, and check-ins.
  present    [CogletId]    The coglets the cogent is paying attention to.
  limit      u8            At most this many are present at once. Default 4.
  revision   u64           Increases on every change.

PresenceChange
  base       u64           The revision this change was made from.
  add        [CogletId]
  remove     [CogletId]
  attention  CogletId?     A new attention coglet. Needs the presence.attention grant.
  limit      u8?           A new limit. Needs the presence.attention grant.
  reason     str?

PresenceRequest
  # A coglet asking for attention. Usually an absent one.
  coglet     CogletId      Filled in by its runtime, never by the coglet.
  urgency    "low" | "high"   A hint to the attention; it decides.
  reason     str

CheckIn
  # What the attention receives about one absent coglet.
  coglet     CogletId
  status     CogletStatus  Read by the router through the manage port.
  held       [Message]     What it sent the attention on its own initiative since the last check-in, oldest first.
  dropped    u64           Messages dropped since the last check-in because its hold was full.

Router
  presence.get() -> Presence
  presence.set(PresenceChange) -> Presence
      Refused with `conflict` if `base` is not the current revision, and with
      `invalid_argument` if the result would exceed `limit`, the same coglet is in
      both `add` and `remove`, or a coglet is not one of this cogent's.
  presence.request(PresenceRequest)
      Delivered to the attention at once. At most one per coglet per minute;
      more are refused with `resource_exhausted`.
  presence.changed(Presence)
      An event to subscribers, and to every coglet that entered or left presence.
  presence.check_in(CheckIn)
      Sent to the attention for each absent coglet at the cadence it set.
```

**Authority is by grant**, as everywhere in the router protocol: each caller
needs a grant for the exact method.

- `presence.get` and a subscription to `presence.changed` are granted like any
  other method.
- `presence.set` is granted by default to the attention coglet and to the owner,
  as the actor. A grant to the owner covers the owner's own requests, whichever
  peer carries them, and not the coglets on that peer.
- Changing `attention` or `limit` needs the separate `presence.attention` grant,
  held by default only by the owner.
- Every coglet is implicitly granted the `presence.changed` events about itself,
  so it always learns whether it is being attended, without a subscription.
- The router holds the manage port's `status` grant for the cogent's coglets, so
  it can fill `CheckIn.status`.

**Holds and check-ins.**

- Each absent coglet's hold is bounded (64 messages by default). When it fills,
  the router sends a check-in early, but at most one early check-in per coglet
  every 10 seconds. While a check-in is rate-limited, the oldest held messages
  are dropped and counted in `dropped`.
- The attention sets each absent coglet's check-in cadence. The default is a
  few minutes. A coglet with nothing held gets a status-only check-in at a
  slower rate.
- A coglet that stops or is removed leaves presence, and `presence.changed`
  says so. Its hold is delivered in a final check-in.
- Removing a coglet that is not present is not an error; it changes nothing.
- When the attention changes, everything held is delivered to the new attention
  in its first check-ins.

### 4. Spec changes

`spec/hub-protocol.md` adds the `presence.*` messages and the
`presence.attention` grant above, and the router telling each runtime the
cogent's presence, as an additive change, in the next minor
version of the router protocol, with CDDL in `spec/messages/` and cases in
`conformance/hub/`. Nothing existing changes meaning. The spec text lands with
the implementation card, not in this ADR's PR.

The design pages drop "the cogent's centre": the README, the cognitive
architecture, coglets and runtime, app integration, and the unforged, staged
integration, LLM and secrets pages say "a Softmax-hosted runtime" where they
meant a place.

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
- **Hold everything an absent coglet sends, answers included.** It would stall
  the attention's own calls to memory and the factory for minutes. Answers are
  never held.

## Consequences

- The cogent design pages and their diagrams are rewritten without a centre, and
  explain presence.
- The fast loop's role is named: it is the cogent's attention.
- Model spend and latency budgets concentrate on present coglets. Absent coglets
  need policies that can run without the cogent answering at once.
- The router protocol grows one additive message family and one grant. Existing
  traffic and spec-v0 fixtures do not change.
- A runtime forwards to the router what it would otherwise deliver locally to
  the attention from an absent coglet, so presence is part of what a runtime
  learns from the router.
- A coglet cannot pull the cogent's attention to itself except by asking, and
  asking is limited to once a minute, so a misbehaving coglet cannot flood the
  attention.

## Verification

New golden fixtures in `spec/fixtures/hub/` for each `presence.*` message and
refusal, and these conformance cases, against both the reference router and the
mock:

- `presence_set_requires_grant`
- `presence_attention_change_requires_attention_grant`
- `presence_set_conflict_on_stale_revision`
- `presence_set_rejects_over_limit_overlap_and_foreign`
- `presence_present_delivers_immediately`
- `presence_absent_holds_until_check_in`
- `presence_answers_never_held`
- `presence_local_delivery_forwards_to_router`
- `presence_follow_up_updates_never_held`
- `presence_attention_change_moves_holds`
- `presence_check_in_cadence_and_early_rate_limit`
- `presence_hold_full_counts_dropped`
- `presence_request_reaches_attention_and_is_limited_per_minute`
- `presence_changed_reaches_entering_and_leaving_coglets`
- `presence_stopped_coglet_leaves_with_final_check_in`
- `presence_changes_no_grants`

## Approval

Pending independent review and the accepting PR.
