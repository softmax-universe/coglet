# Common conventions — 0.1

> **Names on this page.** The prose says *adapter* and *router* ([ADR-0020](../docs/adr/0020-vocabulary.md)). Identifiers, file names and wire names still say `surface` and `hub` until the rename lands; they mean the same things. The runtime's own dispatcher is called the *event router*.

Normative keywords MUST, MUST NOT and SHOULD have their RFC 2119 meanings.
Decision: [ADR-0002](../docs/adr/0002-common-conventions.md) ([PR #4](https://github.com/softmax-universe/coglet/pull/4)).
Other specs MUST import these definitions, not restate them. CDDL rules are
unqualified names in `messages/common.cddl`; JSON Schema references use
`common.schema.json#/$defs/<name>` (for example `event`). A CDDL consumer
concatenates the common rules after its own entry rule; common.cddl's first
rule is the standalone union of common messages.

## Encoding

Wire values and values hashed as CBOR MUST be RFC 8949 §4.2.1 **core deterministic** CBOR:
shortest integer/length encodings, definite lengths, and map keys sorted by
bytewise lexicographic order of their deterministic encodings. This is NOT
§4.2.3 length-first ordering (some libraries call that mode `canonical`).
Only these major types are allowed, recursively:

| Major | Allowed values |
|---|---|
| 0 | unsigned integers 0..18446744073709551615 |
| 1 | negative integers -9223372036854775808..-1 |
| 3 | valid UTF-8 text (no Unicode normalization) |
| 4 | arrays of profile values |
| 5 | maps with unique text keys and profile values |
| 7 | false, true, null only |

Major 2 (bytes), major 6 (tags), all floats (including NaN/infinity), undefined,
other simple values, duplicate keys, trailing data and indefinite lengths are
forbidden. Binary capability data requires a subsequent explicit spec decision.
One frame contains exactly one item; transport specs define framing and limits.
JSON is a debugging/fixture view only, with no tagged conversions. Tools MUST
preserve full integer precision (do not parse counters as JavaScript Numbers).
Records at the negotiated version reject unknown fields; payload/args/details
are open text-key maps but remain restricted to profile values.

## Names and identities

Filesystem state hashes are the Git SHA-256 tree object ids specified in
[the ABI](abi.md#journal-and-trace-one-format), not hashes of CBOR snapshot maps
([ADR-0035](../docs/adr/0035-state-hash-git-tree-id.md)). Files use mode `100644`,
directories `40000`, Git entry ordering and raw 32-byte ids inside trees; ids
elsewhere are 64 lowercase hex. The empty snapshot is the SHA-256 empty tree.
Record, code, manifest and other non-filesystem hashes retain their definitions.

All grammars are ASCII and case-sensitive; regexes match the whole string.

| Definition | Grammar |
|---|---|
| `node-id` | `[0-9a-f]{64}`: the 32-byte iroh Ed25519 public key, lowercase hex |
| `coglet-id` | `coglet:<node-id>` |
| `surface-id` | `surface:<node-id>` |
| `hub-id` | `hub:<node-id>` |
| `component` | `[a-z][a-z0-9_]{0,31}` |
| `capability-name` | `<component>.<component>` (e.g. `speech.speak`, `game.move`) |
| `method-name` | 2..4 dot-separated components (e.g. `debug.data.read`) |
| `correlation-id`, `action-id` | `[0-9a-f]{32}`: opaque 128-bit ids |

Each coglet has its own node key even when hosted by a shared runtime. Prefixes
separate roles, not trust domains. A syntactically valid key is not proof of key
possession or enrollment. Router protocol MUST bind claimed keys to authenticated
transport/delegation and enforce per-method grants; local binding is the adapter
protocol's responsibility. Never authorize by the string alone.

## Envelopes and action lifecycle

Every envelope below has `correlation_id`. Initiators allocate collision-resistant
ids, unique for their identity across restarts; replies retain the request's id.
Unsolicited events allocate a fresh id. Derived events MAY retain a causal id.

- `event`: `source` (any role id), `kind` (capability-name), `seq` (uint64),
  `logical_time` (uint64), `payload` (map), `correlation_id`.
  Sequence numbers start at zero, increase strictly for each source identity,
  and survive reconnect/restart (rotate identity if continuity is lost).
  Logical time is nondecreasing integer microseconds in that source's runtime
  epoch, not Unix/wall time; it is recorded for replay. Cross-source ordering
  and epoch mapping belong to the ABI and the runtime's event router, not numeric comparison of clocks.
- `action`: `id` (action-id), `coglet` (coglet-id), `capability`
  (capability-name), `args` (map), `deadline` (uint64), `correlation_id`.
  Deadline is an exclusive expiry instant in logical microseconds, scoped to
  the boundary carrying the envelope (ADR-0009 M-C): an ABI guest intent uses
  its runtime epoch and remains immutable in the journal. An adapter-wire action
  uses the executing adapter epoch, translated ONCE before first dispatch and
  durably retained with the complete act envelope. `hub.actions` uses the
  RECEIVING runtime epoch, not the router's epoch: the sender establishes that
  mapping and retains the mapped router request before first send; the receiver
  separately translates once at its adapter boundary. Never rewrite a journaled
  intent or retranslate an existing ID on retry/reconnect/new clock samples.
  Missing/stale mapping refuses new dispatch. At or after deadline reject/expire;
  zero is already expired, never "unlimited". Translation MUST reject expired
  input and uint64 overflow rather than wrap/clamp it into a future deadline.
  Retry lookup/reconciliation precedes fresh admission/expiry checks.
- `action-accept`: `id`, `correlation_id`, `accepted` (boolean), and `error`
  iff accepted is false. Rejection is terminal and produces no outcome.
- `action-outcome`: `id`, `correlation_id`, `status` (`succeeded`, `failed`,
  `cancelled`, `expired`); `result` (map) iff succeeded, otherwise `error`.

The hyphenated names above are schema names, not extra wire discriminator keys.
Transport message types select the envelope. Accept is immediate admission,
NOT completion: an admitted action has one eventual terminal outcome, including
failure/cancellation/expiry. When carried as an event, its kind is `action.outcome`,
its payload is the action-outcome and its correlation id MUST match that payload.
The pair (coglet, id) identifies an action. Retries MUST retain the same envelope
and ids; conflicting reuse fails with `conflict`, never executes again. Protocol
specs MUST define durable deduplication, recovery and retention bounds before
claiming exactly-once effects. Schemas check shapes, not these temporal rules.

## Shared errors

`error = {code, message, retryable, ? details}`. `message` is nonempty human
text, not a machine discriminator; `details` is an optional profile map. No
stack traces, credentials or private state in shared errors. `retryable` is a
required boolean indicating whether the same operation may succeed later;
it never authorizes a retry or overrides idempotency rules. Unknown codes are
not valid at 0.1; additions require a negotiated minor version.

| Code | Meaning |
|---|---|
| `invalid_argument` | Bad message, value or name |
| `unsupported_version` | No mutually supported spec version |
| `unauthenticated` | Missing/invalid proof of identity |
| `permission_denied` | Identity lacks the method/capability grant |
| `not_found` | Referenced entity absent |
| `conflict` | Conflicting id reuse, state or exclusive ownership |
| `resource_exhausted` | Budget, queue or rate limit exhausted |
| `deadline_exceeded` | Admission/execution deadline elapsed |
| `cancelled` | Explicit cancellation or kill switch |
| `unavailable` | Transient dependency/transport outage |
| `internal` | Unexpected implementation failure |

## Versioning and negotiation

A spec version is `MAJOR.MINOR`, each a decimal integer 0..9999 without leading
zeros (except zero). This document defines **0.1**; there is no implicit 0.x
compatibility exception. Major changes may break compatibility. Minor changes
MUST be additive only: no removals, changed meanings, stricter bounds or new
required fields in existing messages. A feature introduced at a higher minor
MUST NOT be emitted at a lower selected version.

Both adapter `hello` and router `enroll` requests carry the shared `version-offer`
object `{versions: [spec-version, ...]}` (nonempty; duplicates have no effect). A successful
reply carries `version-selection` `{version: spec-version}`. The responder
selects the numerically greatest (major, minor) pair explicitly present in both
supported lists; versions need not form a contiguous range. The initiator MUST
verify membership before any operational message. No intersection yields
`unsupported_version` and closes the operation without enrollment/side effects.
An enroll over a previously negotiated session MUST offer and select only the
session version; never silently upgrade/downgrade inside that session. A fresh
session renegotiates. Authentication is not replaced by version negotiation.
Protocol specs embed these objects and define their framing/authentication.

## Executable fixtures

`fixtures/common/cases.json` selects a CDDL entry rule / JSON `$defs` rule per
case; valid entries include golden `cbor_hex`. `wire-invalid.json` contains
malformed/non-profile CBOR rejected independently of shape. Run:

```sh
python3 -m pip install -r spec/tests/requirements.txt
python3 -m unittest discover -s spec/tests -v
```

The tests validate every semantic case against both actual schemas and require
byte-identical decode/re-encode for every valid golden CBOR item. Wire-negative
cases separately enforce deterministic/profile restrictions. This does not
claim protocol lifecycle or implementation conformance.
