# Complementary runtime-facing surface-0.2 fixtures

These six cases have harness `peer_role: surface`, so they test the **runtime**.
They are complementary to—not the same files as—the five surface-peer cases in
`../surface02/`. That suite has harness role `runtime` and cannot execute against
the opposite endpoint unchanged. Frozen `surface` and `speech` cases are untouched.

All trusted catalog bytes and the independent pin are shared with
`../surface02-fixtures/app-catalog.json`. Read `{id: 1}`, rename `{title: "new"}`
and the built-in Face action are copied exactly from their surface-peer
counterparts; tests bind provisioning to those source payloads. Wire deadlines
are translated by the real runtime clock and are not asserted byte-for-byte here.
The synthetic harness surface supplies read/accept/outcome responses, not the
runtime bridge. Action cases require pending=1 after accept and pending=0 after
outcome. The valid-read case establishes query issuance and post-reply liveness;
health pending=0 is **not** durable supervisor-result consumption proof.

## Trusted launch inputs

The framing-only production-runtime bridge must receive:

- `COGLET_CONFORMANCE_CONTRACT_PROFILE=surface-0.2` from the harness;
- `COGLET_FIXTURE_CATALOG` and `COGLET_FIXTURE_CATALOG_SHA256`, exactly as for the
  existing selected surface suite (do not derive the independent pin);
- `COGLET_FIXTURE_CONFIG=conformance/surface02-fixtures/adapter-config.json`;
- `COGLET_FIXTURE_RUNTIME_SETUP`, selecting one of the existing-format trusted
  setup JSON files here (`base`, `read`, `rename`, `face`);
- `COGLET_FIXTURE_RUNTIME_BINDING`, selecting `runtime-binding.json` here.

The runtime02-base/read/rename/face fixture labels choose those setup files.
Each case gets a new isolated actual daemon/WAL and authenticated Unix surface
connection, transparently bridged to stdio u32be CBOR. Emit stderr
`COGLET_CONFORMANCE_READY` only after setup/catalog admission and connection.
Use production real time (`fixture_clock=false`). No wire reply synthesis,
app callbacks, schema bypass or inferred profile is permitted in the bridge.
A clean daemon session EOF may end the bridge with status zero; startup, socket
reset and infrastructure/child crashes must be nonzero, not disguised as refusal.
`expect_close` verifies both EOF and zero stdio exit, rejects timeout/bytes/reset,
and uses one unchanged within_ms budget.

No setup has `native_schedule`: these cases cover the actual runtime surface
boundary and receipt routing, not NativeEngine guest execution, replay or real
app effects. Those retain their independently named runtime/native proofs.

**Preparation is not qualification.** No production runtime registration is
published until the actual bridge is built and these cases execute against it.
