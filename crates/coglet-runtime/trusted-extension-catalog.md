# Trusted local extension catalogs (ADR-0017 prerequisite)

This is supervisor provisioning, **not a wire message, guest import, grant source,
or production game registration**. Built-in bindings remain unchanged. A default
`RuntimeService::open` or `SurfaceSession::new` has no extensions. C ABI global
configuration does not install an extension catalog; use the Rust service API or
the production socket daemon below. Linux socket tests do not qualify Apple
sandbox/device execution, native game producers, motor/research effects or SDKs.

## Exact consumer API

```rust,ignore
let catalog = coglet_runtime::TrustedExtensionCatalog::from_pinned_bytes(
    &std::fs::read("catalog.json")?,
    "<64 lowercase hex SHA-256 of EXACT file bytes>",
)?;
let service = coglet_runtime::RuntimeService::open_with_catalog(
    wal_path, &setup, 128, catalog,
)?;
// Attach service to SurfaceSession::with_service or SurfaceListener::with_service.
```

Production daemon (the existing arguments retain their meaning):

```sh
cogletd --surface-service-socket SOCKET BINDING_JSON UID WAL SETUP_JSON \
  --trusted-extension-catalog CATALOG_JSON SHA256
```

A missing file/pin, invalid pin, unknown option or digest mismatch fails before
listening. The exact digest is stored with the WAL; reopening requires the same
catalog bytes/pin. Removing/changing the catalog is not a way to reinterpret
historical receipts. Rollback disables new provisioning; an existing catalog WAL
requires its original configuration for audit. No process-global registration and
no network schema lookup occur.

The small [example](examples/trusted-extension-catalog.json) is deliberately a
**synthetic fixture**, not a complete game/1 catalog. Its exact file pin is:

```
2f3ab80d5b09134e35a2e63e5a3c14e314e61ac12135fa0a1c723aaf02cf79dd
```

Pin reviewed distribution bytes; do not compute a new pin from an untrusted file
at deployment and call that trust. Whitespace changes also change the pin.

## Local artifact v1

Top-level fields (unknown fields rejected):

- `version`: exactly 1; `namespace`: one capability component, excluding built-in,
  reserved and illustrative fixture namespaces.
- `entries`: entry name -> array of allowed exclusive resource names. Entries,
  operations and resources stay in this namespace.
- `schemas`: explicit HTTPS URI -> rule name in `rules`. URIs outside the frozen
  `coglet.dev/spec/messages/` namespace refer to **embedded local IR**, not URLs to
  fetch. Missing rules/URIs fail startup.
- `rules`: generated CDDL IR dictionary, shared in shape with Java
  `Codec.validateWith` and Rust `coglet_types::validate_with_schema`.
- `operations`: operation name -> closed record `{entry, direction, input,
  output?, resources, single_owner, selection?}`. Direction is `event`, `read` or
  `action`. `input`/`output` are keys in `schemas`. Read/action require output;
  events forbid it. `single_owner` must be true. Each required resource must be
  declared on the exact entry, and must also be present in the owner's grant.

Capabilities advertise input URI for events/actions and output URI for reads.
Both directions still validate. One binding grants no sibling operation authority.
Built-in operation/schema overrides are refused. Resource declarations are trusted
policy, not inferred product rules: product owners must pin the reviewed complete
operation/resource table (the example exercises control, overlay and helper).

Supported finite IR forms, with exact keys:

| Opcode | Fields besides `op` |
|---|---|
| `ref` | `name` (local existing rule, no recursive references) |
| `record` | `fields`: array of exact `{name,optional,type}` records, no rest |
| `union`, `intersection` | nonempty `items` array |
| `array` | integer `min`, `max` (0 <= min <= max <= 262144), `item` |
| `primitive` | `name`: `tstr`, `bool`, `null` |
| `range`, `size` | integer `min`, `max` |
| `literal` | profile-valued `value` |
| `regexp` | `pattern` string using the common generated validator semantics |

Unknown opcodes/keys, unresolved references, open maps, unbounded arrays and
recursive rules are refused, including unused rules. Whole artifact <= 1 MiB;
IR nesting <= 64. All payloads retain the common CBOR profile and transport limits;
extension event payloads also have a 262144 encoded-byte ceiling. A structurally
valid but unsatisfiable range/regexp is not a useful product contract; product
schemas still need their positive/negative fixture suite.

`selection` is event-only and exactly `{operation_field,args_field,value_field,
reads:[operation names]}`. Field names are distinct. Reads must be configured read
operations. `game.observation` requires selection: it validates selected read args
AND output (not a union sibling), then requires the current event grant AND the
selected read grant of the single supervisor-bound owner. Observation grant_epoch
must match. No surface read reply becomes an event.

## Trusted local one-shot read plan

Optional `SETUP_JSON.supervisor_reads` is an array (maximum 16) of closed records:

```json
[{"surface":"surface:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  "coglet":"coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "operation":"game.state","args":{},
  "correlation_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","timeout_us":1000000}]
```

This is a product-neutral supervisor sampling seam, not observation-query
registration or a guest-controlled request. It requires the runtime's exact
manifest owner, distinct valid correlation IDs, selected read input schema and
0 < timeout_us <= 60000000. The existing private socket/embedded pump waits for
matching authenticated surface and current operation/resource/owner grants.
Original operation/args/correlation are persisted at service creation and attempt
reservation commits before transport exposure. Valid results are retained in
`supervisor_reads` WAL audit under that original binding; no inbox/wake/event is
created. In-memory trusted callers still use `request_read`/`take_read_result`.
Audit is not guest-readable authority and is not an automatic event producer.

The plan is immutable for this WAL. Each entry is attempted once; interrupted,
expired or disconnected reads remain without a response, and are **not resampled**
on reconnect/reopen. To request another sample use a new supervisor lifecycle,
not reuse of a correlation or silent reset. Queued results remain owner-scoped and
revocation-purged through the existing native read lifecycle.

## Historical results

Persisted action identity/operation/correlation select the result rule. A new
extension result may become an inbox observation only under the original epoch
AND current same-owner operation/resource grant. Late/revoked/reconnected results
remain durable effect audit but create no guest inbox/wake. Exact historical
retries do not re-dispatch effects or later disclose a suppressed result. Frozen
built-in historical receipt reconciliation is unchanged.

## Requirement-to-named-test matrix

Target: `cargo test -p coglet-runtime --locked --test trusted_extension_catalog`.

| Requirement | Named executable proof |
|---|---|
| Production daemon, private AF_UNIX, local URI schemas, exact pins, journal reopen | `CogletdTrustedExtensionCatalogTest` |
| Per-operation sibling args/value, missing event/read grant, wrong source/owner, multi-owner refusal | `CogletdTrustedExtensionCatalogTest` |
| Exact entry/direction/URI/resource advertisement, built-ins-only refusal | `CogletdTrustedExtensionCatalogTest` |
| Missing control/overlay/helper grants with positive controls | `CogletdTrustedExtensionCatalogTest` -> `extra::resource_matrix` |
| Original durable read binding, sibling response refusal, reopen/no resampling/no invented event | `CogletdTrustedExtensionCatalogTest` -> `extra::daemon_reads` |
| Results use original dispatch, sibling outputs refused, pin fence across reopen | `CogletdTrustedExtensionCatalogTest` |
| Revoke, replacement epoch, kill and grantless reconnect retain audit without guest disclosure | `CogletdTrustedExtensionCatalogTest` -> `extra::fenced_results` |
| Original read correlation/operation plus wrong owner/input/output (direct Rust API) | `selected_read_reply_uses_original_correlation_operation` |
| Reserved/built-in overrides, unknown direction/resource/schema/IR, recursive/open schemas | `pinned_loader_rejects_untrusted_or_open_catalogs` |

Frozen regression gates are separate: `check-primary-abi.py` covers only 21 primary
ABI cases; `check-surface.py` covers frozen role-selected negotiation over socket
and C pointers, not operational qualification. `native_operational` exercises
actual C pointers and daemon/WAL lifecycle. Run workspace tests, native-engine
serial package tests, both clippy modes, fmt and diff checks. The fixture extension
target executes no native game code and establishes no acoustic/device qualification.
