# Java surface message SDK

Java 21 generated records and sealed interfaces for direct record unions, with
native strict deterministic CBOR and runtime CDDL validation. Jackson is used only
for the generated schema resource and JSON fixtures, never for CBOR.

```java
import org.coglet.surface.Generated;
import org.coglet.surface.Codec;

var offer = new Generated.VersionOffer(java.util.List.of("0.1"));
byte[] bytes = offer.toCbor();
var decoded = Generated.VersionOffer.fromCbor(bytes);
Object generic = Codec.decode("version-offer", bytes);
```

All integer fields use `BigInteger`; native wire integers cover `-2^63` through
`2^64-1`. Individual schema rules can impose narrower ranges. Generic values are
`null`, `Boolean`, `String`, `BigInteger` (integral Java primitives are accepted
when encoding), `List<?>`, and text-keyed `Map<?,?>`. No bytes, floats, tags,
indefinite forms, non-shortest lengths, trailing bytes, duplicate/unsorted or
nontext keys, or malformed Unicode are accepted. Nesting is bounded to 64 and
schema traversal to 128.

Optional generated components use `Generated.Presence<T>`: a null component means
absent, and `new Generated.Presence<>(null)` means explicitly present null (only
accepted if its CDDL type permits null). Required null fields remain present.

`Generated.Message.toCbor()` validates both the rule and concrete record branch;
`fromCbor()` validates before constructing nested records. Generic
`Codec.encode(rule, value)` / `decode(rule, bytes)` cover every rule, including
recursive profile values. `encodeProfile` / `decodeProfile` expose only the wire
profile without schema constraints. The generated codecs alone do not implement
selected operation validation or surface-session admission.

Run `mvn test` with JDK 21. Tests recursively walk `../../spec/fixtures`, assert
minimum corpus counts (33 golden, 43 semantic-invalid, 17 wire-invalid), and check
byte-identical roundtrips, typed records, wrong variants, integer bounds,
Unicode, container lengths, wire depth, and schema recursion. Generation is
integrated through `codegen.emit_java.generate(rules)`; generated Java and schema
resources must not be edited by hand.

## Transport and receipt foundations

`FrameCodec` validates frozen surface envelopes and implements bounded four-byte
network-order framing with incremental headers/payloads. Its sticky fatal decoder
rejects non-profile CBOR, bad schemas and truncated frames. `requireDirection`
selects the surface or runtime wire direction; it is not operation admission.

`UnixSurfaceListener` binds only below a current-user-owned 0700 directory, creates
a 0600 socket, refuses stale paths/symlinks/replaceable unprotected ancestors, and
checks the runtime user's actual OS peer credentials before returning a connection.
Use a short socket path (at most 100 UTF-8 bytes). No network fallback is provided.
Same-UID hostility remains outside this boundary. A private trusted prefix protects
its interior directories; shared writable ancestors outside that prefix need sticky
protection and root/current-user ownership. Filesystem ACLs or mounts granting
additional access are a supervisor concern and must not undermine those permissions.

One control thread calls `connection.poll(timeout, receiver)`; a timeout is at most
one second so a future session can service frozen health clocks. `enqueue` validates
surface-to-runtime direction, snapshots bytes and bounds outstanding frames to 128,
including a partially written frame. Fatal receive/direction/callback/I/O/overflow
and EOF close the link and invoke the supplied independent native-stop reflex once.
`close` invokes stop without waiting on the control callback, writer or selector.
After a callback closes the connection, remaining coalesced frames are not delivered.
An already-running callback is **not authority**: the native owner must atomically
check its immutable epoch/generation/deadline with every physical effect under the
same fence as stop. Transport callback cancellation alone cannot enforce that fence.

`ReceiptLedger` separately retains exact act envelopes, admission receipts and
terminal outcomes. It holds a lifetime OS lock plus a same-JVM ownership guard,
uses checksummed deterministic CBOR snapshots, forces file data, atomically renames,
and forces the state directory. Provisioning also forces its parent directory.
The 4096-receipt / 32-MiB limits fail closed rather than evicting retained receipts.
Pruning retains terminal/rejected records for at least 24 hours after the later of
deadline and terminal time; pending receipts are never pruned. Returned maps are
copies. I/O uncertainty invokes stop and poisons the instance, even if a rename
already happened. Reopen revalidates identity/checksum, and `recoverPending` records
failed/unavailable outcomes for uncertain accepted actions, never invokes effects.
The caller must stop native work first; the constructor invokes stop before opening
state. Creation is explicit **fresh identity provisioning only**. The supervisor
must keep the identity anchor outside this directory and never recreate lost state
under an old identity. Reopen refuses absent/corrupt state. This is durable dedup
storage, not an exactly-once engine transaction or a native execution API.

### Qualification boundary

Named unit/integration tests: `FrameCodecTest`, `UnixSurfaceListenerTest`,
`ReceiptLedgerTest`, plus unchanged `CodecTest`. Unix tests use real private AF_UNIX
streams, actual peer credentials, fragmented input, fatal disconnects, and a blocked
control-callback/independent-stop test. Ledger tests reopen actual files, inject
failures before/after rename and directory force, and verify a separate JVM remains
locked after a refused same-JVM opener. Fault injection and ordinary reopen tests
are not a power-loss durability claim.

These are SDK foundations, **not a complete operational Surface**. Session
negotiation, health/inactivity, full effective grants, selected operation schemas,
revocation/epoch/generation/deadline fences and shared language-neutral surface
conformance remain to be implemented and qualified. No game catalog, native motor,
producer, recorder listener or guest delivery is registered by these classes.
ADR-0017 additionally requires the runtime's digest-pinned trusted catalog loader
before any production `game.*` negotiation; released rc1 is not sufficient.
