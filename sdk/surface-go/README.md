# Go surface SDK

The `surface` package implements the surface-side protocol on the existing
`types` and `codec` packages. The production AF_UNIX transport currently targets
**Linux** (SO_PEERCRED); the protocol core and generated types are independent of
that transport. This is not hub enrollment: the trusted supervisor binds the
surface identity and allowed coglets, then OS peer credentials authenticate the
local runtime before the surface initiates hello.

## Host integration

```go
// stateDir already exists, is owned by this user, and has mode 0700.
// Provision Create:true ONCE for a new identity; never recreate a lost ledger
// for an enrolled identity. Keep the supervisor identity anchor externally.
s, err := surface.New(surface.Config{
    Identity: surfaceKey, Coglets: enrolledCoglets,
    Ledger: filepath.Join(stateDir, "receipts"), Create: firstProvisioning,
    Stop: device.StopAll, // synchronous, bounded, independent of control/storage
})
if err != nil { return err }
defer s.Close()

op, err := surface.BoundOperation("expression.set_expression",
    device.PrepareExpression, device.ApplyExpression)
if err != nil { return err }
if err = s.Register(op); err != nil { return err }

// Alternatively use ListenUnix(...).Accept() when the surface owns the socket.
conn, err := surface.DialUnix(runtimeSocket, runtimeUID, runtimePID)
if err != nil { return err }
defer conn.Close()
return surface.ServeUnix(ctx, s, conn, surface.ServeOptions{
    Configured: func() error { return s.Grant(effectiveGrants) },
})
```

Register only operations the device actually implements. `BoundOperation` selects
its release-pinned schema binding and minimum resources; `Register` rejects
unknown/reserved names and missing resources. `bindings.json` is a byte-identical
copy of the spec catalog, enforced by `TestCatalogMatchesSpecAndSemanticByteBounds`.
No schema URI is fetched. Grants use generated `types.Grant`; all incoming and
outgoing envelopes and operation args/results use generated validation. Generic
maps use the native codec integer model below, never JSON floating-point numbers.

`Prepare` is side-effect-free. `Execute` is a bounded/nonblocking native handoff,
already under the effect fence: **do not reenter Surface methods or Context.Effect
from either callback**. Return a result map for synchronous success, or nil for
asynchronous work. An asynchronous backend retains the original RequestContext
and calls `Complete(ctx, status, result, code)` from its completion queue. Its
`ItemID` is the original owner-scoped wire action ID, not a native job token.
Call `ctx.Effect(fn)` around each later native effect; it checks the retained
generation, kill latch, lease expiry and exclusive deadline under the same lock
as StopAll. Capture native session IDs too; never obtain a new Context inside an
old provider callback. `Context(owner)` is for starting unsolicited capture;
`Event(context, operation, payload)` uses its original authority.

Serialize custom `Open/Receive/Drain/Tick` loops on one control owner. Prefer
`ServeUnix`: it bounds both output queues, supports fragmented/coalesced frames,
preserves generation tags through partial writes, purges revoked private replies
and observations, and disconnects on a revoked partially written frame. `Poll`
is for bounded control work only. `Kill(reason)` is callable from another thread:
it physically stops before waiting for control/storage, durably latches, cancels
pending work and sends kill then stopped health. Only a local `Reset` while
disconnected clears the latch. No wire message, reconnect or ready health does.

## Durability and limits

- Single-owner, owned 0600 ledger and stable side-lock file in an owned 0700,
  nonsymlinked directory. Each checksummed snapshot is fsynced, atomically renamed,
  then the directory is fsynced. Native work starts only after durable admission;
  terminal results are persisted before delivery. Storage failure disconnects.
- Pending work at restart becomes failed/unavailable, never re-executed. A durable
  running marker is installed before hello; an unclean restart stays kill-latched
  until local reset. Missing/corrupt state refuses open. External supervisor
  identity continuity is essential if the whole state directory is lost.
- Exact `(coglet,id)` retries replay stored accept/outcome payloads, including
  historical epochs, with fresh event sequence numbers. Changed envelopes conflict.
  Rejected requests are retained too. Epochs, seq and read snapshot revisions
  persist; reconnect requires fresh negotiation and a higher grant epoch.
- At least 24h retention after the later of terminal time and deadline. At most
  4096 retained requests, 64 pending actions, a 64MiB ledger, 128 messages per
  control queue (126 normal/reserved-terminal slots plus 2 kill-notification
  slots), 1MiB per wire frame and 60s future action deadlines. Capacity
  exhaustion fails closed rather than evicting live receipts. Repeated reads use
  unique correlations (65536 retained per connection). Health is sent every 1s;
  3s inactivity or blocked output disconnects and cancels work.
- Supply a thread-safe Clock in persistent logical microseconds when integrating
  a supervisor epoch. The default anchors monotonic process elapsed time to Unix
  microseconds; any persisted-clock rollback fails closed (including a backward
  system-clock change across restart). EpochFloor can raise, never lower, the
  externally retained supervisor epoch high-water mark.

## Qualification boundary

The test-only `cmd/conformance` host is a semantic expression renderer and empty
speech device. It consumes supervisor fixture configuration and invokes this
public SDK, not expected case responses. It claims only the surface suite:
**9 applicable cases pass; the 2 runtime-side version cases explicitly skip**.
Required conformance CI builds it before all registry consumers and includes it
in every registered run; required Go CI additionally runs race tests and the same
registry entry with a per-case identity/skip audit.

Native speech stream/chunk/session ownership, media item tables, normalizers,
heard alignment, provider work/handoff and real audio/display qualification belong
to the device backend. The SDK provides identity, schema/byte caps, resources,
durable receipts, completion/cancellation and immutable generation fencing; it
does **not** synthesize audio, infer heard text, implement a provider, or certify
physical speech from wire fixtures. Do not advertise unimplemented operations.
No embedded runtime C ABI qualification, pod or Baby Bear integration is claimed.

```sh
go test ./...
go test -race ./...
go vet ./...
go build -o ../../target/coglet-go-surface ./cmd/conformance
python3 check_conformance.py --binary ../../target/coglet-go-surface --junit ../../target/go-surface.xml
```

Named lifecycle/socket regressions are in `surface_test.go`, `regressions_test.go`
and `transport_linux_test.go`; `TestSurfaceConformance` builds the Go adapter and
runs every frozen surface case through its own registry entry. Python 3.11+ is
needed for that test; no Python package dependency is required.

## Generated message types


`types` contains generated message structs and record-union interfaces. `codec`
implements the native strict deterministic CBOR profile and shared CDDL IR
validator. No external CBOR dependency is used.

```go
import "github.com/softmax-universe/coglet/sdk/surface-go/types"

offer := types.VersionOffer{Versions: []string{"0.1"}}
raw, err := offer.ToCBOR()
decoded, err := types.DecodeVersionOffer(raw)
```

Check errors on every encode/decode. Generic `types.Encode(rule, value)` /
`types.Decode(rule, raw)` support every CDDL entry rule. Generic integers decode
as `uint64` or negative `int64`; lists and maps are `[]any` / `map[string]any`.
`[]byte` / `[]uint8` and byte arrays are rejected as forbidden byte strings;
use `[]uint64` or `[]any` for arrays of integers. Typed nil pointers encode as
null and are rejected when the selected schema requires a record.
Record-only unions expose numbered structs and `DecodeActionAccept` (for
example) selects a validated branch. Optional struct fields use pointers; nil
means absent. An optional `any` pointer can explicitly represent null.

The codec rejects bytes, floats, tags, invalid UTF-8, non-text/duplicate/unsorted
map keys, non-shortest or indefinite encodings, trailing data, and integers
outside `-2^63..2^64-1`. CBOR nesting is limited to 64 and schema traversal to
128. These are message codecs, not transport or temporal protocol conformance.

The validator consumes additive IR v2: intersections validate every branch,
`default` values are metadata only, tuples backtrack over optional/repeated
entries, and array/map/rest occurrence and map-key constraints apply on both
encode and decode. Open records expose a typed `Rest` map for unclaimed keys.
Generic entry selectors accept `file::rule`; uniquely exported qualified and
unqualified names share a Go type alias, preserving native type identity.

Run `go test -v ./...` from this directory. Tests select generated fixture
families using `codegen/contracts.json`; draft-validated families remain owned
by their protocol suites. Adjacent `.json.format` markers explicitly declare
`semantic-vectors-v1` files and every run reports the skipped count. Unknown
markers and malformed case arrays fail. `semantic: true` cases must pass shape
validation even when protocol semantics mark them invalid. Tests retain minimum
corpus counts (33 valid shapes, 43 shape negatives, 17 wire negatives) and check
byte-identical goldens when supplied. Generate with repository `codegen/run`;
don't hand-edit `types/generated.go`.
