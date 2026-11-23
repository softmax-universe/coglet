# Per-coglet policy — DRAFT 0.1

**Status: DRAFT. Freezes at M3, not v0.** Decision:
[ADR-0005](../docs/adr/0005-policy-promotion-tickets.md) ([PR #10](https://github.com/softmax-universe/coglet/pull/10)).
MUST/SHOULD are normative. [Common conventions](conventions.md) govern encoding,
identities, errors and version compatibility. `policy.schema.json` and `policy.cddl`
are the shape contracts. Concatenate `messages/common.cddl` after `policy.cddl`
and select the `policy` entry rule. JSON files are configuration/debug views; signed/hashed representations
use common deterministic CBOR. This draft is not a kernel implementation.

## Ownership and resolution

The fixed kernel reads exactly one policy for each enrolled coglet. An operator,
not a self-editing coglet or factory, installs policy under separate administrative
authority. There is no implicit permissive fallback: absent, invalid or unsupported
policy blocks propose/promote. Policy never grants capabilities or edits the
kernel. Required manifest capabilities still need current surface grants.

Fields (all required, closed records):

| Field | Meaning |
|---|---|
| `version` | `0.1`, independently selected draft contract version |
| `status` | literal `DRAFT` |
| `coglet` | shared coglet identity; exact registry match |
| `revision` | common opaque correlation-id grammar; new immutable id per installation |
| `control_role` | `application`, `fast_loop` or `factory` |
| `gates` | nonempty unique array of gate records below |
| `human_approval` | whether a separately authenticated human approval is mandatory |
| `probation` | observation and rollback configuration below |

Policy installation MUST verify role against the kernel registry, not trust a
self-declared application role. `fast_loop` and `factory` require human approval
in this draft; application policy MAY require it. Gate and approval requirements
are conjunctive. Tickets may ask for more checks, never remove policy checks.
The kernel pins the policy revision in each proposal. Reinstallation invalidates
unactivated proposals; new evidence/reapproval and a new proposal are required.
It MUST NOT weaken or shorten an already running probation.

## Gates and approval

Each gate is `{kind, suite, max_age_ms}`. `kind` is `journal_replay` or
`headless_game`; at most one record of each kind. `suite` is a nonempty immutable
suite/configuration reference; `max_age_ms` is a positive uint64. Replay is always
required. A game coglet SHOULD also require a headless game fork. Non-game
coglets need not invent a game environment.

Replay runs the named journal corpus with recorded LLM responses, seed, event
batches and initial state, and checks the suite's declared assertions/state or
intent hashes. Headless game forks pin the initial world, adapter version and
scenario seed; no actions reach the live world. Each gate report identifies the
candidate, base, policy revision, kind, suite, checked-at kernel time, pass/fail,
and immutable evidence reference. The kernel MUST authenticate the gate service
and verify those bindings; a factory's claim is not evidence. Only `passed`
reports count. Missing, failed, stale, future-dated, duplicate or mismatched
reports reject the proposal. A report age equal to `max_age_ms` is expired.
Required reports and human approval are checked again on promote admission.

Human approval is a kernel-verifiable attestation bound to the propose request's
correlation id, candidate, base and policy revision, with approver and expiry.
The kernel records that request id on the resulting proposal, so approval can be
obtained before the kernel allocates a proposal id and cannot transfer to another
proposal. `approval_ref` is an opaque
reference, never a boolean authorization supplied by the caller. Factory/fast-loop
identities cannot attest as a human. Revocation or expiry before activation
invalidates promotion; the kernel rechecks at the boundary. Credentials and
private transcripts MUST NOT enter policy or audit records.

## Probation and automatic rollback

`probation` contains positive uint64 `window_ms`, `health_interval_ms`,
`health_timeout_ms`, `max_consecutive_failures`, `max_step_failures`, and
`max_budget_violations`. Interval MUST NOT exceed timeout or window; timeout MUST
NOT exceed window. Durations are milliseconds on the kernel's durable monotonic
observation clock, NOT the coglet's replay logical clock. Clock continuity must
survive restart, or observation is uncertain and handled fail-closed.

Probation starts only on the runtime's acknowledged step-boundary activation,
not on propose, branch publication or promote admission. Health samples bind
coglet, generation and running commit; samples for another generation cannot
reset the counters. A health failure is an explicitly unhealthy sample;
consecutive-failure count resets on a healthy sample. Step failures (trap,
compute cap, deadline or invalid output) and budget violations accumulate for
the probation, without reset. The pinned policy's positive thresholds trigger
at **greater than or equal**, not strictly greater than.

The kernel MUST automatically request rollback when any of these holds during
probation:

- consecutive health failures reach `max_consecutive_failures`;
- step failures reach `max_step_failures`;
- budget violations reach `max_budget_violations`;
- no valid health has arrived for `health_timeout_ms`, measured from activation
  or the most recent valid sample (healthy or unhealthy);
- signature/integrity verification fails, or required capability grants are lost;
- restart recovery cannot establish the activated generation, observation clock
  or durable probation/audit state.

At the expiry boundary, evaluate rollback triggers **before** completing
probation. Time passing without sufficient current health is not success. The
kernel atomically marks the live candidate stable only with no trigger and a
valid healthy sample newer than the timeout. It then advances last-known-good.
Outside probation the kernel keeps health monitoring and emergency stop enabled;
automatic rollback outside probation needs a future policy decision, not an
implicit extension of these thresholds.

Rollback restores **code**, not private `/data`, external effects or an old grant.
Candidates MUST preserve state compatibility with the retained last-known-good
code, verified by the gate suite. Incompatible/uncertain restoration stops the
coglet with outputs fenced rather than claiming successful recovery. On first
installation with no last-known-good, any rollback trigger likewise stops it.
A failed rollback cannot start infinite retries or promote the failed candidate.
See [promotion](promotion.md) for generation fencing and audit custody.

## Fixtures and limits

`fixtures/policy/cases.json` includes application and control-role policies,
invalid gates, forbidden fields, timing bounds and missing approval requirements.
`spec/tests/test_control.py` validates every fixture's shape with both actual
CDDL and JSON Schema validators, then separately checks semantic constraints and
golden CBOR. Gate-kind uniqueness (including identical records) is a semantic
constraint, not a shape constraint in either schema. `regressions.json` and
nested-map mutation tests cover closure and duplicate handling.
These tests are fixture validation, not gate execution or implementation
conformance. Later black-box suites must inject health timeouts, threshold equality,
restart during probation, invalid attestations, state incompatibility and boundary
races into both the real kernel and mock.
