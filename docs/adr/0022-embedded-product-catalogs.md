# ADR-0022: Embedded product catalogs and recorded pod command admission

- Status: Proposed
- Date: 2026-11-24
- Authors: Coglet contributors
- Related task: `t_01a2084e0e33453d`
- Related PR: recorded in the accepting review
- Supersedes / superseded by: none; frozen ABI and adapter wire unchanged

## Context

[Operation bindings](../../spec/capabilities/bindings.md) already allow explicitly
provisioned trusted extension catalogs, and require an ADR for a new guest
observation. PR104 (merge `111c1c8a34cb44e2eaef0987b41e61355c5e5c12`) provides
[the digest-pinned local catalog](../../crates/coglet-runtime/trusted-extension-catalog.md)
through Rust and the socket daemon. Its default remains built-ins-only. The
embedded C ABI's `coglet_runtime_start` still calls `RuntimeService::open`, so iOS
cannot opt into the same loader. No peer advertisement can register a product
operation. `hub.inbox` is not exposed by the embedded adapter link; startup files
are not a live command channel.

Baby Bear's iOS PodHost already owns durable command admission, cancellation,
expiry and running-to-unknown recovery. NativeCapabilities implements platform
effects. The first real embedded behavior is routing `system.status` admission
through a product Wasm guest, not a test echo or a speech operation. It must not
open a second actuator, serialize arbitrary telemetry floats into the CBOR
profile, or claim every pod command has migrated. This ADR approves a finite
initial contract, not a cutover or implementation qualification.

## Decision

### Product-neutral embedded provisioning

Add an optional `trusted_extension_catalog` field to the trusted JSON accepted
by `coglet_runtime_start`, not to a wire message or guest import. It is a CLOSED
record `{json, sha256}`: `json` is a UTF-8 JSON string containing the exact catalog
bytes; `sha256` is their reviewed, lowercase 64-hex SHA-256 pin. The existing
16-MiB start-config bound applies, and the loader's 1-MiB decoded catalog bound
also applies. Reject unknown fields, missing members, invalid pins, mismatches
and invalid catalogs before publishing a service or opening a link to it.

Use `TrustedExtensionCatalog::from_pinned_bytes(json.as_bytes(), sha256)` and
`RuntimeService::open_with_catalog`, not a second loader or global mutable
registry. Without the optional field, preserve exactly the built-ins-only path.
The process's immutable `coglet_runtime_configure` identity and C function
signatures remain unchanged. Normal start refusal creates no running service;
process panic fencing and close-all-links-before-stop rules remain unchanged.

The existing WAL catalog pin fence applies: reopening requires the original
catalog bytes and pin. Stopping, disabling a setting or reinstalling the app
must not reinterpret or discard historical extension receipts. A new unrelated
catalog uses a new explicitly selected WAL; old files remain available for audit.
The app bundles reviewed bytes and the separate expected pin. Computing a pin
from arbitrary received bytes is not trusted provisioning.

No built-in binding, generic operation, new runtime event, router RPC, schema
network fetch, or permission source is added. Both default and native-engine
builds support the same provisioning checks; only the latter executes Wasm.

### Product-owned first observation and action

Baby Bear owns a versioned, digest-pinned `babybear` extension catalog, schemas,
positive/negative fixtures and guest in its own repository. Do not put these
operations into Coglet's frozen `bindings.json`. Initially its CLOSED inventory is:

| Name | Direction | Closed payload / arguments | Successful output | Resources |
| --- | --- | --- | --- | --- |
| `babybear.command` | event | `{command_id, command, arguments}` | none | none |
| `babybear.system_status` | action | `{command_id}` | `{}` | `babybear.command_owner` |

`command_id` is a nonempty UTF-8 string of at most 256 encoded bytes, preserving
exact product identity bytes. `command` is the literal `system.status`;
`arguments` is the CLOSED empty map. No generic command string or JSON envelope
permits adding another operation. Both operations belong to one `babybear.pod`
entry, whose only declared resource is `babybear.command_owner`; both are
single-owner bindings. URIs identify bundled product-local closed rules, outside
`coglet.dev/spec/messages/`. All unknown/nested fields, siblings, unsupported
commands and profile violations are refused. The complete event retains the
runtime's extension-event size cap. Future operations require their own design
review/ADR, not widening these records.

A trusted Swift adapter sends the observation only after authenticating a hub
claim, validating target, command bytes and immutable expiry, and durably
retaining its admission/mapping. One private adapter identity is bound to exactly
one permitted coglet. It holds the current event grant. Native platform telemetry
is NOT part of this event. The runtime commits the exact event before real WAMR
entry; the guest observes recorded bytes only, never a live iOS API.

The real guest decides whether to stage `babybear.system_status` for that
command and persists its own cursor/dedup state. It emits an ordinary common
action with the recorded command identity binding, stable action/correlation
identity and runtime-epoch deadline. No use of `test.*`, simulated guest scripts,
or disguised speech. Action IDs/correlations follow existing common grammar;
retain an exact durable product-command-to-action mapping rather than truncating
or treating arbitrary command IDs as valid common IDs. Repeated conflicting
commands fail closed. The existing runtime grant/resource/epoch rules apply.

Swift receives the committed action and checks its original mapping and current
PodHost lifecycle/expiry before any effect. PodHost and CommandJournal remain
the sole platform executor and authoritative Baby Bear terminal ledger. A
successful adapter output `{}` means the mapped native command actually completed
and its original payload is durable; an admission/queue ack is not success.
That payload goes to the hub through the existing receipt projection. It is not
copied into an untyped guest result. Failure uses the existing common error;
unknown product outcomes never become successful adapter outputs.

Retain exact translated action bytes, original command expiry, epoch and receipt
mapping before acknowledgment/dispatch. Query retained identities before fresh
admission. Never rerun an already-dispatched command after an uncertain guest,
transport, fsync, cancellation or restart result. Read-only `system.status` is
not an excuse to weaken this rule for future effects. Guest results use the
existing extension receipt fencing and runtime terminal inbox; the product
journal remains authoritative for result upload and unknown-outcome recovery.

### Native default, shadow, selection and rollback

Native remains the default and only live executor until recorded parity passes.
Shadow uses a separate WAL/identity/binding and an isolated action sink: grant
its resource only in that isolated runtime, collect the actual guest's committed
intent, and never connect the sink to NativeCapabilities or live command upload.
It consumes recorded command/lifecycle facts from the native run. It does not
sample telemetry again or run the native handler a second time. Missing grants
that cause every emit to fail are not meaningful positive shadow qualification.
The process-global C ABI supports only one running embedded service: serialize
shadow sessions; do not invent concurrent global instances.

Selected coglet mode retains one product command ledger and one live executor;
there is no native/coglet race. Close admission and quiesce the old owner before
selection. Switching modes never resubmits running or unknown IDs. On background,
protected-data loss, revocation, panic or failed durability, fence fresh admission
and preserve original receipts. Rollback closes links before stop, selects native
for future eligible commands only and retains all ledgers/catalog pins for audit.
It cannot undo a prior effect or reinterpret an unknown result. A stopped or
failed shadow must not stop the live native executor. No automatic phone cutover,
new audio path, second speaker or background execution entitlement is authorized.

## Alternatives

- Speech/test.echo tunneling: rejected; false capability authority and no useful
  product behavior qualification.
- A new embedded inbox/debug import: rejected; the existing event/action
  transport and pinned loader suffice and retain journal/grant semantics.
- Global dynamic capability registration or a permissive map: rejected; bypasses
  closed operation/schema/resource and WAL provenance checks.
- Passing the complete system.status telemetry through a generic guest map:
  rejected; unbounded/product-private data and floats do not fit this contract.
- A C ABI fork duplicating Rust catalog validation: rejected; one reviewed loader.
- Migrating every native command at once: rejected; each additional effect needs
  an explicit closed product mapping and its own observed parity evidence.

## Consequences

This is an additive trusted-host configuration path and a finite product
observation design, not a changed adapter wire or Wasm ABI. Existing callers and
frozen conformance inputs are unchanged. Product code/catalogs stay in Baby Bear;
only the embedded opt-in and generic tests belong in Coglet. Require a new
immutable iOS release containing this seam before the product pins/uses it.
Archive+SDK link qualification is not simulator execution. This first command is
an incremental real behavior slice, not evidence that all pod behavior migrated.
The original card's simulator unit/UI, side-by-side, rollback, independent review,
CI and landing gates remain open until executed and independently checked.

## Verification

Named tests below are REQUIRED, not claims of tests already passing:

- `embedded_trusted_catalog_start`: actual C pointers in fresh subprocesses,
  valid digest-pinned catalog negotiation plus a committed extension event/action;
  missing/wrong pins, unknown fields, invalid schema/resource/owner, default
  built-ins-only rejection, close/stop/reopen and changed-pin refusal. Execute
  real WAMR in native mode, distinguish reference/no-guest validation separately.
- `embedded_trusted_catalog_failure_fences`: failed startup exposes no service,
  malformed catalog never creates dispatch, panic/status and live-link stop
  fencing remain intact; historical unknown receipts cannot be replayed.
- Existing `trusted_extension_catalog` production socket tests, default/native
  runtime suites, frozen primary-ABI and actual C-pointer adapter regressions.
- Baby Bear `PodCogletContractTests`: selected closed schemas, command identity
  bounds/UTF-8, sibling and float rejection, catalog/guest/release pin mismatch.
- Baby Bear `PodCogletEmbeddedTests`: hosted iOS simulator links the actual released
  framework and positively executes the compiled product guest, original event,
  committed action and durable terminal receipt; no mocked guest/script fallback.
- Baby Bear `PodCogletParityTests`: recorded native and isolated real-guest shadow
  observations match admission/intent/receipt projections; exactly one native
  handler invocation, positive selected-mode command and unchanged hub payload.
- Baby Bear `PodCogletRollbackTests`: duplicate/conflict/cancel/expiry, lock and
  foreground transitions, restart during dispatch, unknown receipt upload,
  grant/epoch loss, guest failure and rollback never repeat an effect.
- Baby Bear `PodCogletLaunchTests`: simulator UI launch/relaunch preserves native
  default, explicit selection and rollback, same durable unknown IDs and no
  fresh dispatch while locked/backgrounded. Positive named execution, zero skips;
  exact tested-source ledger, independent exact-head review and required CI.

Device sandbox/suspend/acoustic qualification remains distinct. Do not label
Linux C-pointer tests, source assertions or SDK links as iOS execution evidence.

## Approval

Proposed only. Link independent exact-head review and the accepting merge before
implementation. Approval of this design alone authorizes neither product cutover
nor claiming simulator/device qualification. No implementation is included here.
