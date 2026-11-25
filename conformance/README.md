# Black-box conformance harness

`./conformance/run --self-test` requires [uv](https://docs.astral.sh/uv/) and
Python 3.11+. The runner has no Python package dependencies. It imports **no
implementation code**. Real and mock implementations register the same suites.
The included loopback claims only `self-test`, never a protocol suite.

```sh
conformance/run --self-test --junit target/self-test.xml
conformance/run --junit target/registered.xml           # every registry claim
conformance/run --impl 'path/to/binary --stdio' --suite surface
conformance/run --socket /path/to/local.sock --suite surface
cargo build --locked --manifest-path conformance/iroh-driver/Cargo.toml
conformance/run --endpoint <64-hex-public-key> --alpn coglet/hub/0 \
  --address 127.0.0.1:12345 --suite hub
python3 -m unittest discover -s conformance/tests -v
cargo test --locked --manifest-path conformance/iroh-driver/Cargo.toml
```

`--impl` is shell-quoted argv parsed with shlex, **not a shell command**. Its
stdin/stdout are protocol bytes; logs belong on stderr. `--socket` connects to an
already running Unix socket. An implementation that normally runs a socket
server can supply a small executable launcher/pipe adapter, not a second set of
implementation-specific tests. Commands and relative fixture paths run from the
repository root; the runner itself works from any directory. A fresh process or
socket connection is made per case, and spawned process groups are cleaned up on
success, failure and timeout. Unix-socket/server state reset is the fixture's job.

Stdio registry entries may set `ready_line = "COGLET_CONFORMANCE_READY"`.
The adapter emits this exact newline-terminated ASCII line on **stderr** after
durable initialization, immediately before emitting its initial protocol frame.
The harness waits for it under `connect_timeout_ms` (default 5000) and the existing
whole-case watchdog. Only then does the unchanged case timeline start; readiness
does not consume, decode or discard stdout. EOF before the marker fails startup;
a missing marker times out. Frame deadlines still apply after readiness, including
the first hello. This separates process/storage initialization from protocol
response latency without retries or fixture changes. Adapters without the field
retain exec-only startup behavior.

`python3 conformance/repeat_surface.py --output target/readiness/normal` runs
50 full in-process reference-surface iterations. Add `--slow-startup` to delay
only the real Init ledger's first `fdatasync` by 250ms with Linux `strace`.
With `--without-readiness`, the same binary and cases must reproduce a step-0
timeout; with readiness all cases must pass. This counterfactual isolates the
harness budget defect; it does not identify the historical CI host's particular
disk or scheduler stall. `surface-readiness` CI preserves per-iteration JUnit and
summary counts on the same `ubuntu-latest` runner type as conformance. The two
runtime-under-test version cases remain unsupported-role skips, not surface passes.

## Case data

Each `conformance/<suite>/**/*.json` is one case object or an array of case
objects. All JSON files under a claimed suite are executable; put descriptive
fixtures/schema data under `spec/fixtures/`, not among executable cases. Names
must be unique within each suite. Unknown fields, missing/empty suites, malformed
registries and duplicate JSON keys fail preflight, before launching any peer.

```json
{
  "name": "error-within-deadline",
  "peer_role": "runtime",
  "fixtures": ["enrolled"],
  "timeout_ms": 10000,
  "steps": [
    {"send": {"example": "request"}, "at_ms": 20},
    {"match": {"error.code": "permission_denied"}, "within_ms": 200}
  ]
}
```

This is a **tooling example**, not a normative protocol message. Protocol cards
supply actual envelope values from `spec/`. Supported step actions (exactly one):

| Action | Meaning |
|---|---|
| `send` | Encode JSON value using the common deterministic CBOR profile, send one frame. |
| `send_hex` | Send literal CBOR bytes, including deliberately malformed wire fixtures. |
| `expect` | Consume the next frame; require exact typed value equality (true != 1). |
| `expect_hex` | Consume the next frame; require byte-for-byte equality. |
| `match` | Consume the next frame; require every dotted-path/value equality; numeric components index arrays. Additional fields are allowed. |
| `journal_hash` | Consume the next frame; compare a lowercase 64-hex journal/state hash. `path` defaults to `state_hash`; the suite specifies the normative hash, never the runner. |
| `silence_ms` | Require no bytes for this interval. EOF is failure, not silence. |

`within_ms` defaults to 1000 and bounds the **whole** frame read/write, not each
byte. No messages are silently discarded while searching for a match. A missing
predicate path fails. Cases can explicitly expect asynchronous events in order.
`at_ms` is a nondecreasing absolute offset from the start of the case timeline,
using a monotonic clock; assertions without it run immediately after the previous
step. `timeout_ms` (default 10000) bounds setup, connection and the entire case;
all durations are limited to 0..300000 ms. Teardown is separately bounded.

### Peer roles and fixtures

`peer_role` identifies **the role the harness plays**, not the implementation
under test. It defaults to `runtime`; values are lowercase names. For example,
a case with `peer_role: "surface"` sends surface hello/events to the runtime under
test; a case with `peer_role: "runtime"` sends runtime messages to a surface under
test. Hub-client cases use `peer_role: "runtime"` unless explicitly testing another
client role. Set an implementation's `peer_roles` to the roles its test peers can
play. A role it does not support is explicitly **skipped**, not passed.
For direct invocation, use repeatable `--peer-role` to set these roles.

`fixtures` is an optional array of implementation-neutral setup labels. Registry
entries map each label to `setup`/`teardown` argv hooks and/or `env` configuration:

```toml
[[implementations]]
name = "example-surface"
suites = ["surface"]
peer_roles = ["runtime"]
transport = "stdio"
command = ["target/debug/example-surface", "--stdio"]

[implementations.fixtures.enrolled]
setup = ["path/to/test-provisioner", "setup"]
teardown = ["path/to/test-provisioner", "cleanup"]
[implementations.fixtures.enrolled.env]
COGLET_FIXTURE_STATE_DIR = "target/example-fixture"
```

Hooks are trusted test configuration, not remote instructions. They run without
a shell, from the repo root, receive no stdin, have a 10-second cap each, and
must exit zero. Stdout is discarded; bounded stderr is included on failure.
Configuration is explicit: only string-valued `COGLET_FIXTURE_*` environment
entries can be set. Hooks and the implementation also receive
`COGLET_CONFORMANCE_CASE`, `COGLET_CONFORMANCE_SUITE`, and
`COGLET_CONFORMANCE_PEER_ROLE`. All selected fixture env values are provided to
all hooks and the implementation; avoid conflicting env names between labels.
Setup runs in label order; teardown runs in reverse order, including the label
whose setup failed. Teardown failure fails the case. Hooks must not daemonize
children: their process groups are cleaned up. Provision external servers in CI
or use hooks to provision state consumed by the spawned implementation.

**Unknown fixture labels produce JUnit `<skipped message="fixture unsupported: …">`
and a SKIP console line, never PASS.** An exit-zero run with skipped cases is
not a full conformance claim. This permits spec cases for enrolled keys,
exact-method grants, one-use invites, hosted delegation and deterministic
boundary triggers to exist before a particular implementation supports them.
Provisioners own the actual test-only state and must not target production data.

## Registry and wire adapters

`impls.toml` contains `[[implementations]]` tables with `name`, unique `suites`,
`transport` and the relevant transport fields. All registered claims run
sequentially, regardless of `--suite`; `--suite` selects only direct invocations.
An empty registry explicitly makes no conformance claims. The old bootstrap
`implementations.json` dispatcher has been retired.

- `stdio`: required `command` argv.
- `unix`: required `socket` path to an existing local server.
- `iroh`: required `endpoint` public key and `alpn`; optional `address` (direct
  IP:port), `driver` executable and `key_file` containing a fixture secret key in
  iroh's hex text format. Alternatively a fixture can set
  `COGLET_FIXTURE_IROH_KEY_FILE`. No private key goes in cases or committed registry.
  Without a direct address the driver uses iroh's N0 address lookup/relays; local
  tests use Minimal endpoints and no external discovery. There is no implicit
  enrollment or grant: the cases send the real protocol operations.

Local pipe/socket framing defaults to `u32be`: a positive 4-byte big-endian
length followed by one CBOR item, capped at 4 MiB. `framing = "cbor-sequence"`
selects concatenated self-delimiting CBOR items instead. These are explicit
**adapter settings**, not normative transport decisions. Encoding follows
`spec/conventions.md`: deterministic CBOR, text-key maps, exact uint64/int64,
no floats/bytes/tags; noncanonical or trailing response data fails.

The standalone Rust `iroh-driver` is a byte bridge. It connects using the
configured ALPN and opens one bidirectional QUIC stream per operation; hub wire
frames remain u32be length + CBOR (maximum 1 MiB). Responses are forwarded
immediately, including multiple reply frames. Authentication is iroh's real
public-key transport, not simulated JSON. Operation streams may overlap;
correlation ids in suite expectations establish causal identity. The helper
limits concurrent operations and applies a 300-second safety cap; case deadlines
normally stop it sooner.

For multi-frame operations (git credit/chunk/end), set `stream` on each send to
the same integer in 0..2^63-1 and `finish: false` until the last send. The last
send uses `finish: true` (the default), half-closing that operation's send side.
Without `stream`, each send opens a fresh operation. Closed stream ids cannot be
reused. Local transports reject these iroh-only controls. The private pipe
control prefix is u64be stream id + one finish byte + payload, inside a u32be
length frame; it never goes on the QUIC wire. Responses contain only wire frames,
not control metadata. The encrypted binary integration test covers two sends on
one operation stream, with an intervening reply.

## Contract profile selection (ADR-0030 section 1a)

`contract_profile` is trusted local provisioning, not wire negotiation. Values are
exactly `surface-0.1` or `surface-0.2`; anything else (empty, wrong case, padded,
non-string) is a configuration error, never a fallback.

- Registry: optional `contract_profile` per implementation, default `surface-0.1`.
- Cases: optional top-level `contract_profile`, default `surface-0.1`. Every case under
  `conformance/surface` and `conformance/speech` is frozen at the implicit default.
- CLI: `--contract-profile {surface-0.1,surface-0.2}`. Omitted means "use the
  registry"; an explicit value must agree with **every** selected registry entry
  (or direct-mode/`--self-test` implementation), else the run fails.
- Preflight: all of the above, plus case-vs-implementation agreement, are checked
  before any peer is launched. A `surface-0.2` case never runs against a
  `surface-0.1`-provisioned peer or vice versa; there is no skip.
- Delivery: the harness always sets `COGLET_CONFORMANCE_CONTRACT_PROFILE` in the peer
  (and setup/teardown hook) environment, overriding any inherited value; omission is
  an explicit `surface-0.1`. Fixture hook env may not set it (only `COGLET_FIXTURE_*`).
  It is never put in a wire frame. Because delivery is launch-time env, only `stdio`
  entries may select `surface-0.2`; attached `unix`/`iroh` peers cannot.
- A pinned catalog is a separate trusted fixture input: `COGLET_FIXTURE_CATALOG`
  (path) with `COGLET_FIXTURE_CATALOG_SHA256` (the independent pin), supplied together.

### `surface02` suite (separately selected)

`conformance/surface02/` holds `surface-0.2` runtime-role cases (version selection,
pinned-catalog read and action, input-schema rejection, built-in-only downgrade to
`0.1`). Their trusted inputs live in `conformance/surface02-fixtures/`
(`adapter-config.json`, `app-catalog.json`). Run only with
`conformance/impls-surface02.toml`:

    conformance/run --registry conformance/impls-surface02.toml --contract-profile surface-0.2

The Python (`python -m coglet_surface.reference --conformance-clock`) and Go
(`sdk/surface-go/cmd/conformance`, built to `target/coglet-go-surface`) adapters read the
environment through their SDK provisioning entry points (`coglet_surface.reference.provision`,
Go `provision()`), then pass `contract_profile`/`TrustedCatalog` to the SDK constructor.

Swift's compiled reference surface consumes the same profile/catalog/config inputs and runs
these **exact same five files**, with production real time (`fixture_clock = false`):

    swift build --package-path sdk/surface-swift
    conformance/run --registry conformance/impls-surface02-swift.toml --contract-profile surface-0.2

The separate registry keeps Python/Go CI independent of a Swift toolchain; native `swift.yml`
builds the executable first, executes both frozen and selected suites, and retains both reports.
Local Swift-on-Linux success does not establish native Mac qualification.

**Peer-role boundary:** `peer_role: runtime` is the HARNESS role. These cases test surface
implementations, not the production Rust runtime. The latter receives `hello` and issues
`read`/`act`, the opposite direction; registering a surface SDK or fabricating surface responses
cannot establish runtime conformance. Complementary runtime-facing `peer_role: surface` cases
must reuse the same pinned catalog and operation payloads and drive the actual runtime.
The Rust surface SDK remains unqualified for `surface-0.2` and is not registered here.
These cases cover wire/dispatch seams only, not all generated `s02-*` rules or native hardware.
The Go reference filters grants only on positively negotiated `0.1` downgrade via the SDK's
`NegotiatedVersion()` accessor; arbitrary `0.2` grant refusal is not retried as downgrade.

## Reports and proof boundaries

`--junit PATH` writes a JUnit XML report (default `target/conformance-results.xml`)
on success, case failure, skip or configuration failure. Case failures include
the step index and bounded peer stderr; XML-invalid control characters are
removed. Any failed case/configuration exits nonzero; skips are counted
separately. CI uploads separate self-test and registered-suite reports even on
failure, and runs Python regressions plus Rust transport tests.

Self-test exercises exact/predicate/timed responses, malformed-wire errors,
golden CBOR and a real SHA-256 of deterministic state bytes. Regression tests
also check deadlines, mismatches, uint64 precision, skipped fixtures, setup and
cleanup, malformed peers, stderr/stdout floods and the same suite over a real
Unix socket. Rust tests exercise multiple operations/replies over actual local
iroh and the complete **Python runner -> driver binary -> encrypted iroh peer**
path. These prove harness behavior, not any surface/runtime/hub implementation's
protocol correctness.
