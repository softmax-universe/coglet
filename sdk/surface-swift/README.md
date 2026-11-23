# Coglet surface SDK for Swift

SwiftPM library for macOS 14+ and embedded iOS 17+ hosts. It uses the existing
`CogletTypes` strict deterministic-CBOR codec and frozen `spec-v0` contracts;
it contains no Mindister/legacy implementation code or third-party transport.

## Host integration

1. The trusted supervisor configures the runtime identity binding, local OS and
   privacy permissions, supported operations and grants. Select only implemented
   `SurfaceOperation.builtins`; reserved/test names are not production operations.
2. Provision a `FileSurfaceLedger(directory:newIdentity:true)` **once for a NEW
   surface identity**, then reopen with `newIdentity:false`. A missing/corrupt
   ledger is fatal: reconcile uncertain actions and rotate identity, never silently
   make an empty ledger. Each identity has one exclusive supervisor-owned instance.
3. Implement `SurfaceNativeBackend`. `preflight` has no effects; `begin` runs only
   after durable admission. Return nil for asynchronous work and call `complete`
   with the captured action owner, ID and grant epoch only after actual rendering
   or provider completion. Synthesis/queueing is not proof of playback. Stop must
   synchronously fence capture/output/provider callbacks and clear private data.
4. Construct `SurfaceSession` with monotonic microsecond clock and the authenticated
   binding. Before native capture/provider generation, call
   `openNativeObservationSession(owner:epoch:)` and capture its opaque token in
   every callback. `publishObservation` validates the selected payload, current
   owner/resources/epoch and native-session token; transcripts also pass the
   revision/final normalizer. All events share the durable sequence allocator and
   bounded queue with outcomes. Replacing/closing the native session fences its
   old callbacks without needing a grant transfer. Never mint a token on arrival
   of a delayed callback. Close it on provider session close or capture restart.
   `start`, send drained CBOR to the runtime, feed received CBOR to
   `receive`, and call `tick` on the non-audio control queue (at least every 10ms
   while active). `tick` checks health, grants and action deadlines. Native hardware
   timers/off-switches remain independent of a hung control queue.
5. On ANY dispatch/transport failure call `disconnect` and close the transport.
   No grants or effects survive reconnect. Session construction advances the durable
   epoch and fails uncertain admitted actions rather than replaying physical work.

All session and transport calls are serialized by the caller on one non-audio
queue; they are intentionally not `Sendable`. A native off-switch FIRST stops I/O
without waiting for that queue, then calls `kill` to persist the latch and notify.
Only the explicit trusted `localOperatorResetForNextConnection` API, after close,
can clear the latch. It does not reopen the connection or install new grants.
Never expose it as a coglet/provider/hub operation. iOS suspension follows the same
stop, disconnect and close path; no background-execution promise is made.

### AF_UNIX

`SurfaceSocketListener(directory:runtimeUID:)` creates a NEW 0700 directory and
0600 socket. It refuses existing paths instead of deleting a service's socket.
`acceptConnection` authenticates OS peer UID (`getpeereid` on Darwin,
`SO_PEERCRED` on Linux), before any hello is trusted. Same-UID hostile processes
are outside this boundary; use product sandboxing if needed. Do not expose it on
a network. The accepted transport owns its fd on successful construction only.

`enqueue`, `flush` and `receive` are nonblocking. Flush tracks partial writes;
write success is not action acceptance. Incremental framing handles arbitrary
fragmentation/coalescing, validates 1 MiB lengths before payload allocation, and
rejects truncated EOF/profile/schema/direction errors before dispatch. Queue
capacity is 128 control messages; overflow closes rather than dropping outcomes.

### Embedded ABI v1

Supply `SurfaceRuntimeABI(open:send:recv:close:)` from the linked runtime's exact
`coglet_surface_*` symbols. The SDK does not dynamically load code or provide a
runtime substitute. The configured application/runtime binding precedes open.
`EmbeddedSurfaceTransport.send` returns false for would-block (no copy/effect;
retry identical bytes). Receive performs NULL/0 size query, then an exact read;
no caller buffer or pointer is retained. Close is idempotent and applies the
runtime's disconnect path. The runtime implementation still needs its own native
ABI conformance; the package tests a native function-table fixture, not WAMR.

## Durability and bounds

The file store fsyncs a private temporary file, atomically renames, then fsyncs
the parent directory before releasing receipts. Exact (coglet,id) retries precede
current epoch/deadline admission and return retained receipts with fresh event
sequence numbers, never repeat effects. A changed full envelope conflicts.
Records are retained indefinitely, conservatively exceeding the 24h minimum;
1024 recorded IDs / 256 MiB bound the store. At capacity the link fails closed
rather than issuing an unrecorded rejection. This is an initial bounded store,
not a high-throughput production database or exactly-once physical-effects claim.

`SpeechObservations` provides UTF-8-byte-bounded transcript revision/final
filtering and owner/epoch-bound append-only alignment spans. Heard text includes
ONLY spans ending at/before actual played time; unknown alignment gives empty
text and stopped items never advance. A native backend must bind these records to
its real captured session/response IDs, revoke them on ownership loss, and expose
`speech.heard` only when that integration is present. The helper's unit tests do
not prove real acoustic alignment or provider cancellation. Product adapters own
VAD, AEC, STT/TTS, stream ordering, provider sessions and independent timers; do not
advertise optional work/history/session operations without those implementations.

## Reference and verification

```sh
swift test --package-path sdk/surface-swift
swift build --package-path sdk/surface-swift
conformance/run --registry sdk/surface-swift/conformance.toml \
  --junit target/swift-surface.xml
```

`coglet-swift-reference` is a test-only stdio/u32be adapter around the SAME session
and codec; it reads the trusted adapter config, not conformance expected responses.
Its backend implements an explicit semantic expression renderer and speech config
snapshot. Optional media routes fail unavailable; unknown streams fail not_found.
It does not claim hardware/provider speech success. Its fixture-only binding must
never be used in a product. The two runtime-under-test scripts are reported as
role skips; all nine surface-under-test scripts execute. `swift.yml` runs this
harness and Swift unit/native buffer/real socket tests on `macos-latest`, uploading
JUnit. The existing generated-code fixture tests remain in conformance CI.
