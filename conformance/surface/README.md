# Surface conformance case data

Normative contract: [`spec/surface-protocol.md`](../../spec/surface-protocol.md).
Each JSON file is a harness case, using the role/fixture interface coordinated
with harness PR #15. These are test inputs, not an implementation pass claim.

- `peer_role=runtime`: the harness drives a surface-under-test. `surface-base`
  provisions the trusted binding and deterministic clock/hello/config/grant in
  [`adapter-config.json`](../../spec/fixtures/surface/adapter-config.json).
  Advertise all provisioned operations with the pinned schema refs; publish
  capabilities revision 1 and grants epoch 1 immediately after hello_reply,
  except grant-epoch-fence starts at epoch 2 per startup_epoch_by_case.
- `peer_role=surface`: runtime-under-test; `runtime-binding` enrolls only the
  fixture surface and supports only protocol 0.1. It does not grant actions.
- `kill-at-100ms`: additionally fires the local off-switch at case time 100 ms.
  Keep the link open and emit kill followed by stopped health with pending=0;
  this idle fixture has no admitted work to cancel. Kill itself announces full
  grant revocation: do not emit a redundant grants snapshot in this fixture.
  No unsolicited periodic health before 1 s.
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

## Fixture clock and runner watchdog (ADR-0021)

The four Rust reference/mock registrations and the Go reference (`surface-go-reference`, via `sdk/surface-go/cmd/conformance` and `ServeOptions.Control`) opt into `fixture_clock=true`.
The harness provides a private Unix socket in a mode-0700 temporary directory
through `COGLET_FIXTURE_CLOCK_SOCKET`; fixture hooks may not supply that variable.
The socket is a test-control binding, not a surface wire message. Each control
request is an eight-byte big-endian microsecond time; the adapter drives the real
core's `tick(now)` and echoes that time only after stdout has been flushed. Each
ordinary stdin frame is likewise acknowledged after real `receive` and flush.
The harness waits for the send acknowledgement before the next clock advance,
so the separate input readers cannot reorder a frame across an advance.
Stdout is drained concurrently from process startup, including while an
acknowledgement is outstanding; otherwise a large reply can fill the pipe before
its flush completes. The unread-byte buffer has a 4 MiB ceiling. Overflow fails
the case explicitly while draining/discarding further bytes to avoid deadlock;
there is no promise to accept arbitrary output bursts. Frames remain limited by
the existing wire reader, and buffered bytes still fail silence assertions.
Malformed/truncated/retrograde controls fail the adapter; there are no retries.

Time starts at zero and advances only at explicit case `at_ms` boundaries (or
through a silence interval). Repeated equal-time ticks are allowed. A jump tests
observation at that supplied time, not intermediate sampling or a real-time SLO;
real monotonic SDK tests remain necessary for health/silence/native kill. The
kill fixture is triggered before ticking/dispatching at or after 100ms; the
post-kill stimulus remains at 110ms. Real frame bodies, durable stores, rejection,
effects and direct/in-process transport paths are unchanged.

For this explicit binding only, send/receive/control operations have a 3-second
wall watchdog; case total bounds remain unchanged. Existing case `within_ms`
values retain their meaning for non-opted-in implementations. The 200ms case
values are not surface-protocol latency promises. No unexpected frames are
filtered, no missing messages retried, and EOF is not silence. Ordinary stdio,
readiness-disabled startup counterfactuals and live-time SDK tests retain their
real-monotonic paths. Disabling readiness in the startup counterfactual also
turns off the clock socket, since connecting it would be another startup wait.

## Embedded ABI coverage boundary

An ABI bridge runs the same JSON cases through send/recv without socket framing.
The wire harness cannot express native pointer/size-query or would-block return
codes. Those are explicit native adapter integration requirements in the protocol,
not Markdown cases counted as executable passes. This gap is reported to the
Tools lane; there is no C ABI implementation in this spec PR.
