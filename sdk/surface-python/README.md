# Python surface SDK

`coglet-surface` is a Python 3.11+ uv package. It builds on the generated
`coglet_surface.generated` records and their strict deterministic CBOR codec.
It implements the **surface** side of `spec/surface-protocol.md` 0.1 and explicit
ADR-0030 surface-0.2 provisioning; it is not a
runtime, hub, speech provider or native embedded-runtime C ABI implementation.

```sh
cd sdk/surface-python
uv run --locked pytest
uv build
```

The existing `python` CI job runs these tests. `test_conformance.py` runs the
unchanged `conformance/surface/*.json` black-box harness against the actual SDK
and durable SQLite reference device through a test-only stdio binding. It uses
ADR-0021's existing acknowledged fixture clock: durable work does not advance
logical case time; clock advances and wire dispatch are acknowledged only after
SDK processing and output flush. Startup has an explicit readiness line. The
unchanged case deadlines are not host latency promises. Every surface-side case
must execute; the two runtime-under-test cases remain explicit skips.

`test_reference_readiness.py` injects 400 ms negotiation stalls and proves a
failed Unix wire cannot kill the supervisor and fill its control backlog to
Linux EAGAIN. `test_live_serve.py` separately exercises the production transport
loop with a real monotonic clock, grant expiry, health, silence disconnect and
local kill notification. Existing SDK deadline/native-stop and Unix credentials,
framing and revocation tests remain in the required Python suite. Functional
clock results do not establish native-device latency or physical-off-switch
qualification. `tests/repeat_registry.py` retains 20 complete functional registry
JUnit reports under two owned CPU burners; this proof is non-required CI.

## Supervisor integration

The trusted supervisor owns identity, permitted coglet bindings, grants, the
ledger path, local permission checks, and adapter callbacks. Nothing in hello
proves identity. Use `UnixListener` to authenticate the OS peer before calling
`serve`; do not serve this on TCP or expose it as an unauthenticated pipe.

```python
import os
from coglet_surface import Operation, Surface, UnixListener, serve

# Native callbacks are bounded and nonblocking. prepare MUST have no effects.
def prepare_expression(args, context):
    pass  # Additional device/session admission checks; raise Rejection to refuse.

def apply_expression(args, context):
    renderer.apply(args)  # Native implementation supplies its own expiry timer.
    return {}            # A successful built-in action result is the empty map.

surface = Surface(
    surface_identity, permitted_coglet_ids, private_state_dir / 'receipts.sqlite',
    create=False, stop=renderer.stop_all,
)
surface.register(Operation(
    'expression.set_expression', 'expression.control', 'action',
    'expression::set-expression', 'surface::empty-map', ('display.face',),
    prepare_expression, apply_expression,
))
try:
    with UnixListener(private_state_dir / 'surface.sock',
                      runtime_uid=os.getuid(), runtime_pid=supervised_runtime_pid) as listener:
        with listener.accept() as peer:
            serve(surface, peer.fileno(), peer.fileno(),
                  configured=lambda: surface.grant(effective_grants))
finally:
    surface.close()
```

Create the parent directory with mode **0700**, owned by the surface user. The
listener refuses preexisting paths and symlinked/public directories, creates a
**0600** socket, and checks Linux `SO_PEERCRED` (UID and optional supervisor PID).
BSD-style `getpeereid` is supported for UID checks; if PID binding is requested
but unavailable, authentication fails closed. Same-UID hostile code is outside
the protocol's isolation boundary. Native iOS buffer/ownership semantics are
not implemented or claimed by this package.

### Local contract profile and pinned catalogs

`Surface(..., contract_profile="surface-0.1", trusted_catalog=None)` retains the
existing built-in and legacy two-component extension behavior by default. Only
the exact explicit string `surface-0.2` enables the additional
`softmax.app.<component>` operation-name alternative and offers wire versions
`["0.2", "0.1"]`. Empty/unknown profiles are errors, not fallbacks. Peer messages
and schema maps cannot select the local profile.

```python
from coglet_surface import TrustedExtensionCatalog

catalog = TrustedExtensionCatalog.from_pinned_bytes(
    reviewed_catalog_bytes, separately_provisioned_sha256,
    contract_profile="surface-0.2",
)
surface = Surface(
    surface_identity, permitted_coglet_ids, fresh_ledger_path,
    create=True, stop=renderer.stop_all,
    contract_profile="surface-0.2", trusted_catalog=catalog,
)
```

The immutable catalog verifies exact bytes, closed artifact version 1 and finite
local schema IR before opening the ledger. It never fetches a URI. Register
catalog operations with the exact input/output **URI strings** from the artifact;
advertisements preserve those URIs. The SDK validates their selected local rules
before prepare/dispatch and before publication, including selected-read event
sibling schemas and the intersection of event/read grants. Existing
`extensions=`/`schemas=` cannot provision app operations or override pinned rules.

Catalog regex rules deliberately accept a portable subset, not the complete
Rust Unicode regex dialect. Use explicit character classes and ordinary
repetitions; shorthand classes/boundaries, inline flags, lookaround,
backreferences, nested/set-algebra/POSIX classes and possessive repetitions fail
catalog admission rather than being interpreted with Python's different
semantics. The exact counted universal form `^[\s\S]{min,max}$` is supported.
This restriction does not change frozen generated validators. Entry/resource
names remain two-component under both profiles.

The existing SQLite ledger atomically retains the catalog bytes, digest and
normalized profile on creation. Reopen requires the same catalog/profile pair;
profile-less legacy ledgers mean exactly `surface-0.1`. A change requires a fresh
explicitly selected ledger, never rewriting old receipts. A wire `0.1` downgrade
keeps the provisioned catalog but exposes only built-in Face/speech operations;
it clears old connection grants/queued publications and requires fresh grants on
reconnection. SDK manifest admission is not implemented; manifest-format profile
proofs belong to runtime consumers.

`tests/test_contract_profile.py` includes the applicable ADR-reserved local
profile proofs plus authenticated fragmented AF_UNIX 0.2 reads, retained-catalog
reopen and live 0.1 Face/speech actions. Frozen 0.1 generated IR/catalog files and
the existing conformance cases remain unchanged.

### Ledger and clocks

Provision a **new identity and fresh file** once with `create=True`. Reopen it
with `create=False` on every restart. The SDK requires an owned 0600 regular
ledger and holds an exclusive process lock. It uses SQLite WAL with FULL
synchronization. Missing/empty/corrupt ledgers and identity mismatches fail
closed: reconcile uncertain effects and rotate identity, never recreate a lost
ledger under an old identity. Keep the ledger and its WAL on durable local
storage in the private directory; never delete/replace them while running.

Full deterministic action bytes, accepts/rejections and terminal results are
committed before acknowledgements. Exact retries recover historical receipts
before current admission checks and never repeat an effect. Conflicting reuse
is rejected. Event sequence, grant epoch, snapshots, kill latch and the last
surface clock survive restart. Uncertain pending work is stopped and recorded
failed/unavailable during recovery. Receipts are conservatively retained
**indefinitely** (satisfying the minimum 24-hour window); production operators
must budget storage. There is no unsafe automatic ledger eviction.

The default clock is monotonic microseconds. Inject `clock=` for a simulator;
it must preserve the surface epoch across restarts. Rollback fails closed.
After a host reboot invalidating that epoch, reconcile and rotate identity.
The runtime, not this SDK, maps its action deadlines into this surface epoch.

### Adapter API and threading

* `open()` produces hello; `receive(cbor)` consumes one unframed runtime message;
  `drain()` returns surface messages. Framed sockets use `serve()` instead.
* `register(Operation(...))` installs a release-pinned input/output binding and
  resource requirements before open. Built-in direction/schema bindings cannot
  be overridden. Explicit trusted `extensions=` and generated-IR `schemas=` may
  add simulator contracts; network schema fetching and permissive fallbacks are
  never used. `runtime.*`, `action.*`, `llm.*` authority cannot be added this way.
* `prepare(args, context)` validates device/session state **without effects**;
  raise `Rejection(code, message)` before acceptance. `execute(args, context)`
  receives immutable coglet, epoch, correlation, deadline and action-ID context.
  Return a schema-valid map for immediate completion, or `None` for pending
  action work. Marshal `complete(context.coglet, context.action_id, result=...)`
  onto the control thread; terminal callbacks are idempotent and late success
  cannot revive cancelled/expired work. Reads return a bounded map immediately.
* `grant(entries)` installs a full local-policy snapshot with a persisted higher
  epoch. It must already intersect hub policy with OS/user permission. Invalid
  snapshots leave current authority unchanged. Replacement conservatively stops
  and cancels all old-epoch work; `remove(name)` publishes revocation before the
  higher capability revision.
* `event(coglet, name, payload, epoch=retained_epoch)` checks the configured event
  binding, lease and resources. Private speech adapters must also fence by their
  retained stream/session ID and owner before calling it. Runtime routing must
  intersect subscriptions with grants; no private event broadcast is authorized.
* `tick()` advances expiry and health (called automatically by `serve`). Admission
  bounds future deadlines to 60 seconds, pending actions to 64 and each control
  queue to 128 messages. Frames are bounded to 1 MiB. Malformed frames, lifecycle
  violations, queue overflow and heartbeat loss stop devices and fail closed.
* `kill(reason)` applies native stop first, latches persistently, revokes authority
  and notifies the runtime. Only trusted local `reset()` while disconnected,
  followed by fresh negotiation and grants, can clear it. No wire call can.

Serialize SDK access on **one control thread**. Callbacks must not wait for I/O
or run user code; this is not a Python sandbox. A physical off-switch must call
the native stop primitive independently, even with a hung control thread/full
queue, then marshal `kill()` to record/notify. Native effects must implement
independent duration/deadline stops; do not rely on Python polling for safety.
Transport revocation discards unsent private frames, disconnects if one was
partially sent, and retains historical action outcomes for reconciliation.

## Reference-device scope

`ReferenceDevice` is an explicit no-visual semantic expression renderer plus
an empty speech device configured from the specification's adapter fixture.
It applies/resets expression state and expiry, returns the pinned speech config,
and rejects unknown streams/sessions. It advertises the fixture's provisioned
operations but refuses provider-dependent speech work as unavailable. It does
**not** claim the speech suite, real audio, native renderer timing, hardware
kill responsiveness, or provider crash qualification. Those remain adapter/M1
integration gates. `python -m coglet_surface.reference` is an optional test-only
stdio adapter; fixture configuration/ledger environment variables are mandatory.
