# Draft router peering — 0.1

Decision: [ADR-0046](../docs/adr/0046-router-peering-and-authority-intersection.md).
This contract is **draft-validated**, not enabled by hub 2.0 enrollment. No serving
implementation is qualified by these fixtures. Existing deployments keep the
single-router interim topology until separately authorized integration.

Schemas: [CDDL](messages/peering.cddl), [JSON Schema](messages/peering.schema.json).
Import common conventions unchanged. ADR-0046 defines authoritative transport,
mutual pins, authority/disclosure checks, deadlines, retention and outage rules.
No peering value is legal on a frozen or current hub request root.

| Frame | Direction | Meaning |
|---|---|---|
| `peer.enroll` | each router → other | Offer version with exact household, local/remote router and attending pins |
| `peer.enrolled` | responder → initiator | Selection only; neither installs grants nor completes reverse enrollment |
| `peer.route` | origin router → target router | One-hop capability-selected inbox payload; retained origin actor/ticket |
| `peer.accepted` | target router → origin router | Durable admission, not effect completion |
| `peer.failure` | responder → initiator | Common bounded non-disclosing error |

Every reply retains request correlation. `peer.enroll` sender/receiver MUST match
authenticated connection/local identity and local bilateral pins. Admission needs
successful enrollment in **both** directions on this connection. Same request ID
retries retain exact bytes; a negotiation retry cannot change the selected version.

Route `origin` is the authenticated originating coglet, `target` the receiving
service coglet; `actor` is the exact originating authenticated node key. Optional
`ticket` is omitted, never null. `deadline` uses receiving runtime logical time;
zero is expired. `payload` must additionally match a pinned capability catalog.
`private` is optional in shape only: retained classification requires it for
private observations; `{source, owner, epoch}` must match current retained
ADR-0038 bindings, with epoch at least one. Network claims confer no authority.
No absent field is an exemption from privacy. Public/unknown data classification
is not inferred merely from absence of `private`.

`peer.accepted` contains no result payload. Results are subsequent separately
authorized capability messages, subject to the same owner/epoch and receipt
fences. Neither reply nor common error details may leak private data. A structured
failure is possible only when the responder safely identifies the request;
otherwise reset without disclosure. Reset/FIN/timeout is not cancellation.

## Executable evidence

`fixtures/peering/cases.json` has deterministic CBOR shape goldens and negatives.
`fixtures/peering/authority.fixture` is JSON-syntax **semantic** data, loaded by
`test_hub.HubFixtures.test_peering_authority_intersection_and_owner_negatives`.
Its trusted state snapshots deliberately live outside request payloads. Both
actual validators accept every semantic-negative request; the spec-only oracle
then rejects its authority state. This is not real iroh, durable Store, local kill,
privacy-transport, offline runtime or exactly-once effect qualification.

Run full `python3 -m unittest discover -s spec/tests -v`, with the pinned Ruby
reference installed, and `cargo test -p coglet-types -p coglet-proto`. Draft schema
validation does not add a generated consumer or alter frozen contracts.
