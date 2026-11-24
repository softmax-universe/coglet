# Operation bindings — 0.1

> **Names on this page.** The prose says *adapter* and *router* ([ADR-0020](../../docs/adr/0020-vocabulary.md)). Identifiers, file names and wire names still say `surface` and `hub` until the rename lands; they mean the same things. The runtime's own dispatcher is called the *event router*.

Normative pre-freeze amendment: [ADR-0009](../../docs/adr/0009-spec-v0-freeze.md).
The machine-readable [bindings.json](bindings.json) is the exhaustive release-pinned
speech/expression, sandbox ([ADR-0016](../../docs/adr/0016-sandbox-world-bindings.md))
and ABI LLM operation catalog. Its `family:rule` references
select `spec/messages/<family>.cddl` and the corresponding JSON Schema `$defs`.
It imports, rather than duplicates, those records. A valid root union is NOT a
valid operation binding. Entries/resources are not operations.

## Selection and responses

For each adapter event validate `payload` against `input`; for a read or action
validate `args` against `input`. Reads validate `read_reply.value` against `output`
only for `ok=true`. For succeeded actions validate `action-outcome.result` against
`output`, using the persisted original action, not an untrusted claimed operation
in a reply. Failed replies/outcomes use common `error`, not the successful result
rule. Every adapter action in this catalog returns the CLOSED empty map `{}` on
success: application observations arrive through the defined events/reads, not
invented result fields. `speech.config`, `speech.work_state` and `expression.state`
take the CLOSED empty map; `speech.heard` takes `heard-query` and its returned item
MUST match the requested item. Correlations and action IDs retain the common
rules. A reply without a retained request/action binding fails closed.

Capability advertisement still uses one URI per operation: pin it to the
catalog's input rule for events/actions and output rule for reads, using
`https://coglet.dev/spec/messages/<family>.schema.json#/$defs/<rule>`.
The release-pinned catalog supplies the other direction and successful action
result. Both directions MUST be checked; do not resolve arbitrary network URIs.
Unknown extension operations require an explicitly configured trusted catalog
and grant; they are not implicitly authorized by their syntax. Unsupported
operations fail admission, never select a permissive profile-map fallback.

Reads are runtime-issued adapter observations, not a synchronous guest import.
The v0 guest observes recorded subscribed events and inbox messages only; a read
reply MUST NOT be invented as an unspecified guest event. Product adapters needing
a new guest observation must define its contract through an ADR.

## LLM boundary

`llm.complete` is the only built-in ABI `llm` capability in v0. Its `input` is the
whole ABI `llm-request` (including capability, request ID, model route and input),
not common `action.args`; its `output` is `llm-reply`. Unknown LLM capability names
are unsupported unless a later ADR defines them. The runtime commits request
and outbox before handing to the configured authenticated gateway adapter. This
is NOT an adapter `act`, and NOT a new router-protocol wire method. Phase 2 gateway
adapters must implement the existing ABI boundary; any new inter-host gateway
wire protocol needs an ADR before use. Never tunnel it through an unrelated router
method. Adapter setup binds requester identity, grant and route outside provider
input. Provider input/output remain profile maps, not new grants or endpoints.
The gateway records the one terminal success/failure and retained request and
correlation IDs before `hub.inbox` delivery; the runtime uses ordinary `llm.reply`
inbox materialization and wake semantics. Missing/uncertain provider receipts
require reconciliation, not blind resends or synthesized success. Replay only
uses recorded replies. Real dispatch/spend/crash qualification is an M1 gate.

## Reserved and illustrative names

All `runtime.*` names are reserved to the local runtime, never advertised as
adapter operations, minted by an external producer or accepted merely because a
payload claims runtime origin. Defined event names are `runtime.inbox`,
`runtime.timer`, `runtime.debug_step`, and terminal inbox
`runtime.action_rejected`. Their source is the target coglet; the ABI defines
their exact payloads and authorized triggers. `runtime.receipt` and `runtime.heard`
are audit methods, NOT events, capabilities or remotely callable RPCs. The latter
records aligned heard history at a reserved runtime-owned file boundary; it does
not turn an adapter read into a guest event. Unknown runtime names
are invalid at v0. `action.outcome` is the common terminal action event, not a
capability grant: bind it to the admitted action and authenticated adapter (or
separately retained synthesis authority). `llm.reply` is gateway terminal inbox
data, not an adapter event or a self-authorizing capability. None permits an
external sender to mint runtime wakes, bypass grants or change private owners.
M-A trace tests establish structural/source binding only; M1 must prove authority.

`sensor.tick`, `motor.move`, `other.write`, `hub.note` and `test.*` (including
`test.echo`) are illustrative fixture operations, not production catalog entries.
A fixture adapter may install a local test-only catalog/grant for them, isolated
from production dispatch; parsing a manifest/envelope is insufficient. `speech.speak`
is a historical grammar example, NOT an alias for `speech.speak_chunk`. Marker
entries `speech.hands_free` and `speech.push_to_talk` expose no operations.

## Tests and remaining gates

`test_contract_bindings` checks the selected JSON/CDDL rules, golden bytes,
unknown/missing fields and valid-sibling negatives. It also checks exact direction,
empty result/read arguments, the generic-envelope bypass and reserved/test-name
exclusion. `surface/bindings.fixture` is semantic binding data, not a codec corpus
with a universal entry rule. These checks are specification tests, not a runtime.
[The M1 inventory](../../docs/reviews/spec-v0-m1-gates.md) identifies the owners of
authentication, temporal behavior, provider and native implementation qualification.
