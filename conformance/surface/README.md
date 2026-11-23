# Surface conformance case data

Normative contract: [`spec/surface-protocol.md`](../../spec/surface-protocol.md).
Each JSON file is a harness case, using the role/fixture interface coordinated
with harness PR #15. These are test inputs, not an implementation pass claim.

- `peer_role=runtime`: the harness drives a surface-under-test. `surface-base`
  provisions the trusted binding and deterministic clock/hello/config/grant in
  [`adapter-config.json`](../../spec/fixtures/surface/adapter-config.json).
  Advertise all provisioned operations with the pinned schema refs; publish
  capabilities revision 1 and grants epoch 1 immediately after hello_reply.
- `peer_role=surface`: runtime-under-test; `runtime-binding` enrolls only the
  fixture surface and supports only protocol 0.1. It does not grant actions.
- `kill-at-100ms`: additionally fires the local off-switch at case time 100 ms.
  Emit kill before stopped health; no unsolicited periodic health before 1 s.
  The fixture must not clear the latch in response to a wire action.

Each case runs on an isolated fresh connection/state directory. IDs are synthetic
public fixture identities, never production keys. Hooks select config by
COGLET_CONFORMANCE_CASE and supply it through COGLET_FIXTURE_* settings.
Missing fixture mappings/unsupported roles must SKIP, never PASS. Native methods
must be exercised by the same implementation adapter as real operation, not by
returning the expected script verbatim. No mock/runtime exists in this spec PR.

Cases exercise version selection/failure, read snapshot, accept→outcome,
per-coglet grants, wrong grant epoch, deadline equality, ID dedup/conflict,
unknown streams and latched kill. Registered real and mock surfaces run the same
case files. The harness's framing buffer ceiling is not the protocol's ceiling:
the surface itself enforces 1 MiB. Framing vectors (zero, oversize, truncated,
trailing CBOR and bytes) live in `spec/fixtures/surface/framing.json` and are
checked by the fixture tests; a future native transport adapter must reject them.

## Embedded ABI coverage boundary

An ABI bridge runs the same JSON cases through send/recv without socket framing.
The wire harness cannot express native pointer/size-query or would-block return
codes. Those are explicit native adapter integration requirements in the protocol,
not Markdown cases counted as executable passes. This gap is reported to the
Tools lane; there is no C ABI implementation in this spec PR.
