# ADR-0039: Coglets live in unforged repos; hubs and smart routers

- Status: Proposed
- Date: 2026-11-25
- Authors: David Bloomin (design), David's Claude Code session
- Related PR: the publishing PR
- Supersedes / superseded by: supersedes the split between the router and the
  fast loop in [ADR-0036](0036-distributed-cogent-and-presence.md) (the router is
  now the cogent's attention), and the direction of
  [ADR-0031](0031-manage-port-and-filesystem-patches.md) §2–§5 (no Git inside a
  coglet). The implementation order of
  [ADR-0035](0035-state-hash-git-tree-id.md) and the
  [staged integration](../cogent-design/unforged-staging.md) page still applies:
  this ADR sets the target, not the schedule.

## Context

The cogent design had a runtime that stored each coglet's files its own way,
a router that only carried messages, a separate fast loop that did the
cogent's thinking, and a Softmax app that was the one place a runtime ran on
the user's machine. [Coglets on unforged](../cogent-design/unforged.md) showed
that coglet state, steps and patches map directly onto unforged's git objects,
and ADR-0036 removed the centre.

Four things are still awkward: routing and attention are split across two
coglets that must always sit next to each other; nothing names the bundle of
coglets that every runtime needs to join a cogent; the Softmax app is special
for no protocol reason; and "coglet" is one shape, when we will want several
kinds of router, database and so on.

## Decision

### 1. A runtime exposes unforged repositories

A **runtime** hosts one or more **unforged repositories** and runs coglets
inside them. It is the unforged daemon plus what must stay fast and local: the
execution engines and their metering, the step loop, grant checks and the
dispatcher. **Any app can host a runtime**, by embedding it or by running
`cogletd` beside it. The Softmax app is one host among many.

### 2. A coglet is a user in a repository

Each coglet is a **user** of the repository it runs in:

- it has an identity (a key), and signs the ref updates it makes;
- it has its own namespace of refs, `refs/users/<coglet>/…`, holding its `state`,
  `policy`, `published` and `inputs` refs as described in
  [Coglets on unforged](../cogent-design/unforged.md#a-coglet-is-a-ref);
- it sees its own tree as its filesystem (`/code`, `/data`, `/logs`), and nothing
  of other users' trees unless granted;
- its grants are per user: which adapters and tools it may use, which other
  users' refs it may read, and who may push to its `policy`.

A step is a commit and a patch is a merge, as that page describes. **Skills
arrive by patch**: a new skill is a commit on the coglet's `policy` ref, merged
between steps.

### 3. Coglets have kinds

Coglets come in **kinds**, and a kind has several implementations: several
routers, several databases, several Faces, several factories. A kind fixes the
messages a coglet provides and calls and its manage contract; the
implementation is its policy. Kinds named now: router, optimizer, factory,
memory, database, Face, app coglet and controller.

### 4. Adapters are mounted like devices

An adapter is **mounted** into the repository, under `/net/<adapter>/`, as
[Secrets and adapters](../cogent-design/secrets.md#adapters-as-mounts) describes.
Listing it reads its catalog; writing a call stages an action; the outcome comes
back in the next input. (Not `/dev`: in unforged `/dev` is the git directory.)

### 5. The router is smart, and it is the fast loop

The **router** carries messages and patches between coglets and runtimes, and it
is also the cogent's **attention**: what ADR-0036 gave to the fast loop. It
answers present coglets, decides what to forward, files tickets, applies
patches, sets presence and reads check-ins. There is no separate fast loop
coglet. Presence works as ADR-0036 specifies, with `Presence.attention` naming a
router.

### 6. A hub is a router and an optimizer

A **hub** is a **router** plus an **optimizer** that improves it: it watches the
router's routing and its fast-loop decisions, files tickets, and patches the
router's policy (and, where granted, its own). Every runtime that joins a cogent
runs a hub; hubs peer with each other, so a cogent is **a set of peered hubs and
the coglets attached to them**. There is still no centre: a hub on a
Softmax-hosted runtime is one hub among several.

On a **device**, the runtime runs a hub, a **Face**, and whatever coglets manage
that device's adapters.

### 7. Face watches and delegates

**Face** listens to gestures and voice and records the screen, with the user's
permission and within the adapter's grants. It can drive an app's interface a
little, for small and obvious things, but mostly it **asks the right coglet** to
act: the app coglet for the app in question, through the router.

### Spec changes

None in this PR. The router-protocol changes (routers peering, users and
namespaces on the git transport) and the ABI changes (coglet as repository user)
come with their own ADRs when the staged integration reaches them.

## Alternatives

- **Keep router and fast loop separate.** They always sit together and the
  router already sees every message; splitting them adds a hop and two places
  to optimize.
- **A coglet as a ref, not a user.** A ref has no identity or grants of its own;
  a user does, and the repository's existing permission model then covers
  coglets.
- **The Softmax app as the only device host.** Ties the design to one app;
  any app that can embed a runtime should be able to host coglets.
- **One coglet shape.** Different routers and databases need different
  policies behind the same messages; kinds make that explicit.

## Consequences

- The design pages drop the separate fast loop: the router takes its messages
  (`feature.request`, ticket calls, `presence.*`), and the optimizer moves into
  the hub.
- Every runtime that joins a cogent runs a hub, so even a small device runs a
  router and an optimizer. A device may run a light optimizer, or none, and rely
  on a peer hub's optimizer; how light is open.
- The Softmax app page describes one host, not the host.
- ADR-0031's plain-filesystem patches remain the near-term implementation; this
  ADR is the target that a later ADR in the staged integration will make
  normative.
- Open questions: how hubs choose which router is a cogent's lead attention when
  the user moves between devices, and how far a device optimizer may patch its
  own router.

## Verification

Design-only. When the staged integration implements it: a coglet's refs are
readable only under its own user namespace without a grant; a patch on `policy`
by an ungranted user is refused; two router implementations pass the same
router conformance cases; a device runtime joins a cogent by running a hub.

## Approval

Pending independent review and the accepting PR.
