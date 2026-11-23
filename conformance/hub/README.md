# Hub wire conformance cases

`cases.json` uses the shared harness interface: `{name, steps:[...]}` with
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
  batch can commit step one. `single-step-requires-pause` starts running.
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
