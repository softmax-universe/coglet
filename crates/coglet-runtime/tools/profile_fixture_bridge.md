# Production profile fixture bridge (Linux)

Launch with `python3 crates/coglet-runtime/tools/profile_fixture_bridge.py`.
The executable is supplied prebuilt, not built or discovered by the launcher:

```sh
cargo build --locked -p coglet-runtime --bin cogletd
export COGLET_FIXTURE_COGLETD="$PWD/target/debug/cogletd"
export TMPDIR="$HOME/.hermes/cache/scratch"
export COGLET_CONFORMANCE_CONTRACT_PROFILE=surface-0.2
export COGLET_FIXTURE_CONFIG=/absolute/path/to/surface-owner-metadata.json
export COGLET_FIXTURE_RUNTIME_SETUP=/absolute/path/to/trusted-setup.json
export COGLET_FIXTURE_RUNTIME_BINDING=/absolute/path/to/surface-binding.json
export COGLET_FIXTURE_CATALOG=/absolute/path/to/catalog.json
export COGLET_FIXTURE_CATALOG_SHA256=EXACT_64_LOWERCASE_HEX_SHA256_OF_CATALOG_BYTES
python3 crates/coglet-runtime/tools/profile_fixture_bridge.py
```

All referenced files must already exist. `COGLET_FIXTURE_COGLETD` must be an
absolute executable path. Profile selection accepts exactly `surface-0.1` or
`surface-0.2`, defaulting to the former. Catalog and SHA256 must be supplied as a
pair, or both omitted for built-ins only. Digest/schema admission belongs to the
real daemon; a wrong pin cannot produce readiness. No pin is normalized.

`COGLET_FIXTURE_CONFIG` is opaque, existing harness surface/owner metadata. The
bridge checks its existence but does not translate it into runtime setup,
effects, grants, subscriptions or bindings. The trusted setup and SurfaceBinding
files are passed unchanged to the daemon. Their authority and correctness are
the supervisor's responsibility; conflicts are refused by production admission.

Each invocation creates a fresh mode-0700 `cf-*` directory under explicit
`TMPDIR`, with short socket `s` and WAL `w`. There is no fallback to the system
temporary directory. An overlong socket path fails explicitly: supply a shorter
scratch root (Linux pathname limit: 107 encoded bytes, excluding the NUL).
The exact daemon command is:

```text
COGLET_FIXTURE_COGLETD --surface-service-socket SOCKET BINDING UID WAL SETUP
  --contract-profile PROFILE [--trusted-extension-catalog FILE PIN]
```

Startup is bounded by a 20-second monotonic deadline. The bridge connects to the
actual child's Unix socket and verifies Linux `SO_PEERCRED` against **both** the
launched PID and effective UID before printing `COGLET_CONFORMANCE_READY` on
stderr. Other platforms fail closed. Socket existence is not readiness. The
daemon independently authenticates the connecting bridge UID. Daemon stderr
remains diagnostic stderr; daemon stdout and stdin are not protocol channels.

Stdin/stdout carry the existing u32be-length-prefixed CBOR stream, byte for byte.
The bridge does not decode CBOR or inspect frame lengths; production `cogletd`
retains framing/size/protocol validation. Each direction has at most 65,536
queued application bytes, independent of frame size, with nonblocking writes and
backpressure; OS pipe/socket buffers are additional kernel-bounded storage.
There are no semantic branches, synthetic replies, clock overrides, case-name
lookups, guest execution callbacks or action fakes. All clocks are production
realtime. Caller EOF, daemon disconnect, broken output, SIGINT, SIGTERM or SIGHUP
ends the invocation, terminates/reaps its isolated daemon process group, and
removes its private socket/WAL directory. EOF is termination, **not** a request
half-close: keep stdin open while awaiting replies. As with other user-space
supervisors, SIGKILL of the bridge or host loss cannot execute cleanup handlers.
This fixture intentionally does not retain a recoverable WAL after termination.

## Executed qualification

```sh
cargo build --locked -p coglet-runtime --bin cogletd
python3 crates/coglet-runtime/tools/test_profile_fixture_bridge.py
```

Tests use the actual rebuilt daemon and the existing no-effects trusted setup
fixture plus the pinned example extension catalog; no native engine is needed.
Named tests:

- `test_selected_02_exact_bytes`: fragmented input prefix/payload, selected 0.2,
  exact response bytes compared to a separately launched real socket daemon,
  no unsolicited response, private directory/WAL and EOF child cleanup.
- `test_default_selected_01_exact_bytes`: the same production route with the
  default 0.1 ceiling, even when the peer offers 0.2.
- `test_wrong_pin_refused_without_ready_or_response`: actual daemon pin refusal,
  nonzero exit, no readiness, no stdout and no leaked private directory.
- `test_bad_selectors_fail_closed`: invalid profile and malformed pin.
- `test_disconnect_has_no_fabricated_response`: invalid CBOR reaches production
  refusal; disconnect emits no synthesized reply and reaps the child.
- `test_sigterm_reaps_actual_daemon`: signal teardown removes the live child and
  private directory.

Scope: this is transport/launch/negotiation evidence, not an app implementation,
full surface conformance run, native-engine/guest qualification, action execution,
crash-recovery proof, or Apple authentication qualification. No conformance,
workflow, SDK, or existing runtime-source files are changed.
