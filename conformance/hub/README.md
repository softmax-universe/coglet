# Hub wire conformance cases

Each `*.json` file is one shared harness case: `{name, steps:[...]}` with
`send`, `match` (dot-path equality) and `within_ms`. No mock implementation is
provided or registered by this spec change. `spec/tests/test_hub.py` verifies
case names, frame shapes and schemas; that is fixture validation, **not** a
claim of wire behavioral conformance.

## Adapter contract

Use iroh ALPN `coglet/hub/0` and u32-big-endian framed deterministic CBOR,
maximum frame body 1048576 bytes. Open one bidi stream when a `send` is a
request (`method` field). Subsequent `send` transfer frames with the same
correlation ID go to that **same stream**, not a new stream. Git sends carry
explicit numeric `stream` IDs outside `send`; `finish:false` keeps request/chunk
sends open, and `finish:true` on the end frame sends FIN. Case-level `peer_role`
is `hub` or `device`; `fixtures` names the isolated `hub.<case-name>` setup.
Unknown fixture labels MUST produce SKIP/incomplete, never PASS.
`peer_role` is the harness's role: `device` drives a hub under test; `hub`
drives a device under test. Hub-role cases also require `hub.reverse-stream`:
the device dials its pinned hub, then the harness opens operation streams on
that established connection. The initial dial-only iroh helper cannot run
these cases until it gains explicit accept/reverse-stream support. They must
SKIP until a registry implementation advertises this fixture; a direct inbound
connection to a production device is not a substitute for the outbound contract.
Read credit/chunk/
end/reply frames in order. Finish after terminal reply; no stream reuse. This
requires the helper to retain operation-to-stream routing, not merely open a
stream for every JSON line. Invalid framing vectors live separately in
`spec/fixtures/hub/framing.json` because their wire bytes have no profile-value
view. The helper must preserve uint64 values exactly.

## Isolated fixture provisioning

Each named case is a fresh, isolated fixture, never production enrollment.
Provisioning is a launch/test-control concern, **not** additional hub methods.
Do not add fixture-only methods to the ALPN. The harness owner maps case names
to these profiles before opening the connection:

- Test-only Ed25519 seeds: coglet `01` repeated 32 bytes, hub `02` repeated,
  device `03` repeated. Actual public keys and signed statements are in
  `spec/fixtures/hub/signatures.json`; never use their seeds in deployments.
- Default: device enrolled, pinned hub, delegation and exact grants installed,
  one stopped coglet, completed step zero, empty inbox and no queued work.
  Establish enrollment before operational cases. Requests sent hub→device use
  the pinned hub's authenticated side of the device-opened connection; events
  and health use the device side. Do not impersonate the sender by a JSON ID.
- Enrollment cases start unenrolled, with open window, correct-device invite
  `11` repeated 32 bytes, except the explicitly expired/closed/wrong-node cases.
  `enroll-reconnect` starts allowlisted on a fresh connection with no session
  selection. `start-before-enroll` has not negotiated on its connection.
- `lifecycle-start-stop`: valid loadable guest and all manifest capabilities.
  `start-missing-capability`: one manifest capability withheld.
- `read-grant-is-not-write`: only `debug.read` is granted and `/data/test.txt`
  contains `original`. Path rejection cases have write authority but no path
  authority for traversal/inbox. `write-stale-snapshot` has write authority but
  current snapshot hash is not 64 `e` characters; reject before admission.
- `pause-single-step`: a running counter guest, completed step zero; one empty
  queue yields a recorded runtime.debug_step wake and can commit step one. `single-step-requires-pause` starts running.
- Inbox cases use a stopped coglet whose idle boundary queue drains immediately.
  Exact retries return the persisted receipt, not another line. The suite's
  post-run oracle MUST parse inbox JSONL and assert exactly one ABI inbox-message,
  index zero, matching id/sender/event and one escaped newline inside payload;
  no raw newline splits the event. This semantic oracle is required in addition
  to the current script's `eof` check, not satisfied by it.
- `inbox-boundary-queued`: guest is held in a safe, bounded in-progress step so
  append is durably queued; cancellation occurs before its boundary. Post-run
  oracle asserts no append, with a cancelled receipt. `queue-backpressure` holds
  256 pending mutations, no slot available, no append admitted by this request.
- `cross-owner-receipt`: operation `88` repeated 16 bytes belongs to another
  authenticated actor; do not disclose it. `unknown-receipt` has no such ID.
  Both return `not_found` with the same error shape for the caller (apart from
  request correlation), even with receipt and original-method authority. Missing
  grants are not a substitute for this ownership check. See ADR-0032.
- Event cases start watermark zero; identical batch is accepted once, sequence
  two is a gap. `action-expired` has authority/clock mapping, but deadline zero
  is already expired: no surface dispatch or subsequent action-outcome.
- Git cases use the real self-contained bundle and archive in
  `spec/fixtures/hub/git.json`. They start with no hub ref and an authorized
  uninstalled target. Initial credit is the complete declared size (small enough
  for the 64 KiB allowance), to make the credit assertion deterministic. A push
  stages verified code; subsequent install binds it and leaves it stopped.
  The empty core Wasm fixture is a transfer artifact, not an ABI execution guest.
  Invalid-signature case must fail before credit; hash/offset failures leave
  no ref, private-state or installed-coglet changes. Quarantine is cleaned.

## Observation regression fixtures

`read-*`, `fetch-*` authorization cases and `status-*` use the same pinned hub
transport identity; local scopes differ. For `actor-isolation`, ticket `77`
repeated 16 bytes belongs only to the hub actor, not the coglet actor supplied
by the request. For `ended-ticket`, that ticket belonged to the requesting
coglet actor but has ended. For `missing-ticket`, only ticket-scoped authority
exists; no non-ticket grant exists. All reject before any private data or fetch
metadata. `scoped-allowed` grants the coglet actor ticket `66` repeated 16 bytes
the exact method and path/ref, with `/data/test.txt` containing `original`.
The fetch allowed control checks admission metadata only, not completed transfer;
the fixture must cancel/clean its outstanding transfer after the assertion.

`spec/fixtures/hub/fetch.json` contains actual Git bundles and coglet-key-signed
A/B/C statements for both published and logs refs, plus a correctly signed
competing generation-3 statement (same new tip, different old tip).
`fetch-{published,logs}-{catch-up,identical,equal-conflict,older}` drive a hub
observer. `catch-up` seeds durable generation 1/A; all other modes seed generation
3/C. Provision the peer binding and fetch authority, then trigger one fetch after
enrollment with correlation ID `aa` repeated 16 bytes. The harness receives that
reverse-initiated request and maps its stream to tooling ID 100 (not QUIC's native
stream number). `hub.reverse-fetch-observer` MUST be unsupported/SKIP until the
adapter implements this mapping AND the following post-run oracle:

- catch-up: accepted C, watermark 3, statement C, one accepted-observation audit;
- identical: same C and watermark, no new accepted-observation audit;
- equal-conflict: competing statement rejected as conflict, C unchanged;
- older: generation 2 rejected as conflict, C unchanged.

For the two negative cases, the hub MUST reject signed metadata before granting
credit, reset the operation and record the local conflict result; the fixture
oracle waits for that terminal result and asserts no transfer credit/disclosure
or tracked-ref mutation. For positive cases grant the complete small bundle size
as initial credit; the oracle waits for verified durable import after end/reply,
not merely for the last harness send. Reopen the observer store before checking
watermarks, and assert quarantine cleanup for rejected transfers. The current
harness cannot exercise these reverse-stream/post-run hooks; shape CI and the
fixture decision oracle are NOT a claim that these scenarios run on a hub.

## Publication/rewrite fixture contract

`publish-rewrite-{record-and-replay,failure-no-promotion,substitution-rejected}`
requires `hub.publish-rewrite-oracle`. The wire script establishes the enrolled
device session; **that reply alone is not this test's acceptance**. After enrollment,
the test-local hook invokes the hub's build/propose API (not a fixture-only ALPN
method), drives the resulting signed push through the normal device receiver,
and waits for terminal publication and replay results. The adapter must support
these post-run observations before advertising the label; otherwise SKIP.

- `record-and-replay`: compile an ABI-valid guest containing a NaN-producing
  arithmetic operation and bit-exact reinterpret/store of its result. Pin the
  ADR-0003 tool/policy under test; assert original and rewritten bytes differ.
  Record actual digests and source commit. Assert lint/rewrite precedes gates,
  gates name rewritten digest, and signed archive contains exactly that output
  plus matching `hub-build-record`. Verify hub and later coglet publication
  signatures. Execute one batch, then replay with compiler/rewriter disabled:
  ABI code_hash/blob and /code provenance must identify the rewritten bytes,
  and recorded outputs/state hashes must agree. This case does not qualify
  every engine/architecture or turn the proposed ADR into accepted policy.
- `failure-no-promotion`: inject a rewrite failure, then separately a gate
  failure after successful rewrite. Both must leave prior signed refs, live
  commit and executable unchanged; no promotion signature for either candidate.
- `substitution-rejected`: after successful build, substitute original bytes
  at bin/step.wasm, first under the old signed archive (signature/digest rejection),
  then in a correctly signed test archive retaining the old provenance (output
  hash rejection). Both leave live code unchanged. Separately replace or omit
  the rewritten replay blob; replay must fail closed without compiling, rewriting
  or loading the original. Assertions must distinguish these rejection stages.

The checked-in Git fixture uses empty core Wasm and a test-only identity transform
record to test provenance shape/hash/signature binding. It is not the NaN guest,
an ABI execution guest, a qualified production policy, or execution of these
future kernel/engine hooks. `test_build_provenance_binding` checks its digests;
`test_real_git_bundle_and_archive_digest` verifies the actual signed archive.

## Required stateful observations

The implementation adapter MUST expose test-local journal/dispatch observation
outside the protocol, or a granted debug snapshot, for assertions not encoded in
simple reply matches: once-only action dispatch, inbox line count, no data in git,
and audit origin. Never count a success reply as proof of these effects.
Both the real runtime/hub and the mock must be driven by the **same** cases and
oracles. A skipped/unimplemented oracle is an explicit incomplete result, not a
pass. No such implementation is claimed by the current spec fixture CI.

Before v0 implementation conformance is declared, extend this case set with the
harness owner's crash/reconnect and scheduling controls to cover: crash between
receipt/audit/data commit; grant revocation between queue and boundary; debug
write while stepping and before/after snapshot hashes; action admission loss and
unknown physical-effect recovery; signed published/log fetch replay; log batch
rotation; git credit decrease/oversend/stall and expansion-limit rejection;
control-stream starvation; health loss/degradation and no direct sibling routing.
These requirements are normative in the protocol, but are not falsely presented
as already exercised by the shape-validation tests.

## Regeneration and tests

With `spec/tests/requirements.txt` installed (Python 3.12), run:

```sh
python3 spec/messages/hub/generate.py
python3 spec/messages/hub/vectors.py
python3 -m unittest discover -s spec/tests -v
```

Generators are deterministic for the checked-in schemas and test-key signing
statements. Git pack compression may differ across Git versions; CI verifies the
checked-in bundle itself with `git bundle verify`, strict object validation,
archive reconstruction, SHA-256 digests and Ed25519 signature verification. It
does not regenerate packs and mistake a Git-version difference for a violation.
