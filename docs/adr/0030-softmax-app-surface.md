# ADR-0030: Softmax app state and semantic controls adapter

- Status: Proposed (design only; no implementation qualification)
- Date: 2026-11-24
- Authors: Coglet contributors
- Related task: `t_01a20fdfde58e6ba`
- Related PR: recorded in the accepting review
- Supersedes / superseded by: none; additive product catalog and explicit naming amendment

## Context

David asks that coglets see and control all Softmax app state: current views,
focus, controls and settings. The app lives in `apps/softmax-mac/`, including
SwiftUI views, AppKit-owned panels, a menu bar extra, onboarding, Face, chat,
push-to-talk, HUD and orb. Its existing speech adapter is not an app automation
contract. Driving arbitrary accessibility APIs or putting UI data in speech
payloads would bypass the operation bindings and duplicate the app's actions.

[ADR-0009 M-C](0009-spec-v0-freeze.md) and
[operation bindings](../../spec/capabilities/bindings.md) keep runtime-issued
reads distinct from guest observations. [ADR-0022](0022-embedded-product-catalogs.md)
and the [trusted catalog loader](../../crates/coglet-runtime/trusted-extension-catalog.md)
provide explicit, digest-pinned product contracts; peer advertisement is not
provisioning or authority. The app currently permits only a small speech/Face
operation set in its runtime host. Merely registering Swift callbacks will not
make new operations usable by a real guest.

There is also a naming mismatch to resolve explicitly: the frozen
[capability grammar](../../spec/conventions.md#names-and-identities) permits
exactly two components. `softmax.app.*` operations have three. This ADR proposes
the narrow additive alternative below before any generated schema or code changes.

## Decision

### 1. Product ownership, names and provisioning

Add a versioned, reviewed `softmax` trusted extension catalog owned by
`apps/softmax-mac/`, not new built-ins in `spec/capabilities/bindings.json`.
Its ordinary entry is `softmax.app`; its separate sensitive-action entry is
`softmax.app_sensitive`. Each operation retains its own grant. The ordinary
entry is not a wildcard granting every operation with a matching prefix.

Amend `capability-name` to accept its existing two-component grammar OR exactly
`softmax.app.<component>`, using the existing ASCII component rule. Do not widen
other namespaces or accept four components, trailing dots or prefix matches.
This applies consistently to common event/action names, manifest subscriptions,
required capabilities, grants, catalog advertisements and generated validators.
It does not change `method-name`, identities, envelope fields or clock domains.
Introduce this additive grammar in negotiated surface protocol minor `0.2`, not
by silently changing the accepted language of `0.1`. Keep `0.1` validators and
fixtures unchanged; `0.2` admits all prior two-component names plus the exact
new alternative. A session selected at `0.1` cannot advertise, grant or publish
new app operations. Offer `0.2` only when both runtime and adapter implement its
validators; old-only peers negotiate `0.1` with app operations unavailable, not
a flattened alias. Version-aware validation must precede new-name dispatch.
At negotiated `0.1`, Face and speech operations remain usable unchanged; hiding
app operations must not remove their advertisements, grants, observation channels
or action paths. The same Face coglet must continue to run unchanged on Mac,
Echo Dot and EO1 bodies, including Echo Dot/EO1 adapters that remain on `0.1`.
App catalog provisioning must therefore permit a built-in-only `0.1` session,
not refuse the entire session merely because the bundled app catalog needs `0.2`.

Python, Go and product-owned Java SDKs and the conformance harness keep their
existing `0.1` defaults and contracts. Until a consumer implements version-aware
validation and negotiation, it must not offer/select `0.2` or admit new app names;
it continues to interoperate on `0.1` with Face/speech intact. A consumer opting
into `0.2` must validate the selected version's names and operation schemas before
dispatch, retain prior two-component operations and refuse unknown profiles.
Generating new types alone does not qualify a consumer's `0.2` support. Any
needed Python/Go/Java/harness implementation is tracked by separate follow-up
owner cards filed by the orchestrator after this docs-only ADR merges; this PR
changes none of those implementations.

The implementation must update the common CDDL/JSON Schema sources, compiler
IR and every generated consumer, plus handwritten grammar checks such as
`sdk/surface-rust/src/ids.rs`. Keep explicit version-specific entry rules rather
than replacing a shared regexp globally. Local manifests/catalogs using app
names require the new contract profile; built-in manifests remain accepted.
Match the new arm as `softmax.app.` plus exactly one component, never
`startswith("softmax.app")`: that also matches unrelated entry/resource names.

Bundle closed CDDL/JSON Schema sources, generated local schema IR, catalog bytes
and a separately reviewed SHA-256 pin. Provision that catalog in the embedded
runtime through ADR-0022. Extend `sdk/surface-swift` with a generic immutable
trusted-catalog input: its current `SurfaceSession` admits built-ins only and
does not yet have product extension support. The new input is optional and
product-neutral; omitting it preserves the existing built-in-only public API,
initializers, defaults and behavior, including `0.1` negotiation. Face, Baby Bear
Mac/iOS and other callers must not supply an empty product catalog or change
source to keep their current operation paths. The SDK must load the same closed
artifact format/limits as the runtime, verify its separately supplied digest,
validate all rule references and reject overrides before opening a session.
Add selected input/output validation against local generated IR, with exact
advertised schema URIs; never delegate validation to untrusted backend callbacks.
The app supplies generated product rules, not a permissive SDK escape hatch.
Prove malformed IR, wrong pins, unknown names and sibling payloads fail closed.
Register beside, not inside or as a replacement for, speech.
No dynamic guest registration, network schema lookup or permissive profile map.
Generate Swift types from these spec sources through codegen; do not handwrite
wire models or copy generated built-in bindings to pretend they include the app.
Retain the existing catalog/WAL pin fence; upgrades must use an explicitly
selected new WAL when pins differ and preserve the old audit history.

### 1a. Local contract profile selection (clarification)

This section fixes the provisioning contract for all consumers before they split
implementation. The APIs and fields below are REQUIRED additive implementation
work, not a claim that current releases expose them. This clarification changes
no frozen `0.1` source, schema, catalog bytes or conformance case.

Four independent identities MUST NOT be conflated:

| Identity | Meaning and selection |
| --- | --- |
| Surface wire minor | The session's negotiated `0.1` or `0.2`; it selects wire validation and the operations usable on that connection, not local provisioning authority. |
| Manifest format `version` | Selects the manifest format. In particular, `"0.2"` already selects `database::db-manifest` in runtime admission/execution; it MUST NOT be reused to select app naming. |
| Catalog artifact `version` | Remains exactly integer `1`, with the existing closed artifact fields, limits and digest over exact distribution bytes. No `contract_profile` member is added to the artifact. |
| Local `contract_profile` | Trusted provisioning context, exactly `surface-0.1` or `surface-0.2`. Omission means `surface-0.1` and exactly the prior `0.1` behavior. |

The local profile selects the name-validation rules applied to manifests and
catalogs, including subscriptions, required capabilities, operation names and
local grant/binding validation. It does not select a different manifest format:
first select the existing format by `version`, then validate its name-bearing
fields under the provisioned profile. Thus database manifests keep their database
semantics under either profile; ordinary manifests also need explicit
`surface-0.2` provisioning before using app names. `surface-0.2` adds only the
narrow naming alternative in section 1, retaining prior two-component names.
It does not widen resource/entry authority or grant app operations automatically.

Only the trusted supervisor/product configuration may set the profile, before
manifest/catalog validation and before a session opens. Never infer it from a
manifest's contents or `version`, artifact names/URIs, generated types, peer
`hello`/caps, grants, or the highest offered wire minor. A guest-supplied manifest
cannot opt itself in. Unknown, empty or malformed explicit values fail closed;
there is no fallback from an unsupported explicit profile to `0.1`. The Go zero
value exception below represents omission, not an accepted serialized value.

#### Exact consumer seams

Use these names in the implementation follow-ups. Serialized provisioning uses
`contract_profile`; language APIs use the explicit spelling below. The catalog
argument always denotes verified immutable catalog bytes plus their separately
supplied digest, never a peer advertisement or an unchecked callback map.

| Consumer | Required local API / field and compatibility rule |
| --- | --- |
| Rust runtime | Add `ContractProfile::{Surface01, Surface02}` and `TrustedExtensionCatalog::from_pinned_bytes_with_profile(raw, sha256, profile)`. Existing `from_pinned_bytes(raw, sha256)` remains the `Surface01` wrapper. `RuntimeService::open_with_catalog` takes the profile retained by the catalog for manifest admission and WAL recovery; default catalog/open paths remain `Surface01`. Add trusted `SETUP_JSON.contract_profile` for setup/manifest admission (including catalog-free startup); when a catalog is supplied its profile must match the setup profile, with omission selecting `Surface01`, never inference from the catalog. |
| Daemon / embedded runtime | Add `cogletd --contract-profile surface-0.2`, supplying the same trusted setup field and catalog-loader profile; omitted flag means `surface-0.1`. The embedded C start JSON uses `setup.contract_profile`, outside `trusted_extension_catalog.json`; the catalog loader receives that value. Keep C signatures and existing catalog `{json, sha256}` record unchanged. Conflicting selectors fail before publishing/listening. |
| Swift SDK | Add `SurfaceContractProfile.surface01` / `.surface02` with raw values `surface-0.1` / `surface-0.2`; add defaulted `SurfaceSession.init(..., contractProfile: SurfaceContractProfile = .surface01, trustedCatalog: TrustedExtensionCatalog? = nil, ...)`. Add `TrustedExtensionCatalog.init(json:sha256:contractProfile:)`, defaulting its profile to `.surface01`; session and catalog profiles must match. These are generic immutable inputs; existing built-in-only initializers/call sites remain source-compatible and behaviorally unchanged. |
| Python SDK | Add keyword-only `contract_profile="surface-0.1"` to `coglet_surface.surface.Surface.__init__`, and `trusted_catalog=None` for the verified immutable catalog input. The selected profile governs local validation and durable identity; existing `extensions`/`schemas` arguments cannot imply `surface-0.2` or bypass pinned-catalog admission for app names. Existing calls retain `0.1`. |
| Go SDK | Add `ContractProfile string` and the optional verified catalog input `TrustedCatalog` to `surface.Config`, consumed by existing `surface.New(Config)`. `ContractProfile == ""` is only the Go zero-value spelling of omitted configuration and normalizes to `surface-0.1` before persistence; all other values must be the two exact profile strings. Existing config literals remain valid. |
| Java SDK (mindister-owned) | Add the trusted configuration builder method `SurfaceConfig.Builder.contractProfile(String)` and optional verified catalog input `SurfaceConfig.Builder.trustedCatalog(TrustedExtensionCatalog)`. The builder defaults the profile to `surface-0.1`; explicit strings use the same strict validation. This names a required new SDK seam in mindister, not an assertion that such a builder already exists in coglet. Existing built-in construction remains available unchanged; the session consumes the built immutable configuration. |
| Conformance harness | Add `--contract-profile {surface-0.1,surface-0.2}` (default `surface-0.1`) to `conformance/harness.py`, and optional per-implementation registry field `contract_profile` with the same default. A CLI override must agree with the selected registry entry; conflicting selection fails before launch. Pass the selected value into trusted implementation provisioning, not wire frames. Keep existing `conformance/surface` and `conformance/speech` cases unchanged at `0.1`; add separately selected `0.2` cases. |

An unimplemented consumer retains its existing `0.1` defaults and refuses explicit
`surface-0.2` selection; adding a configuration spelling or generated types alone
must not enable `0.2`. A profile is a local validation ceiling, not wire
negotiation: a `surface-0.1` configuration offers only `0.1`; `surface-0.2` may
offer `0.2` only after its validators/dispatch are implemented, and also supports
the unchanged `0.1` fallback. Selected wire rules must still run before dispatch.

#### Persistence, recovery and downgrade

Persist normalized `contract_profile` alongside the catalog digest in the
existing catalog/WAL state record (runtime `ServiceState.catalog_pin` gains its
sibling `contract_profile`), in the first durable state before exposing service.
SDKs retain the same pair with the catalog in their existing durable ledger;
no second journal, sidecar, catalog artifact field or network profile is created.
For the catalog fence, replay identity is **(catalog digest, contract_profile)**,
not digest alone. Existing code/manifest/setup pins and other recovery fences
remain additional requirements. No-catalog state retains an absent digest plus
its selected profile. Legacy WAL/ledger records lacking the profile mean exactly
`surface-0.1`; explicitly selecting `surface-0.2` cannot reinterpret them, even
with identical catalog bytes. Unknown saved profiles, mismatched supplied
profiles, removed catalogs or changed digests fail recovery. Omitted and explicit
`surface-0.1` are the same normalized identity. A profile/digest upgrade uses an
explicitly selected new WAL/ledger and preserves the old audit history; it never
rewrites old receipts or infers authority during replay.

A peer negotiating `0.1` does not change this stored identity. Keep the
provisioned app catalog and its `surface-0.2` identity stored, but advertise and
grant only built-in Face/speech operations on that downgraded connection. App
reads, actions and events remain unavailable (including queued publications and
retry dispatch); a saved grant or matching prefix cannot restore them. Do not
flatten names, erase the catalog, relabel the WAL as `surface-0.1`, or refuse the
entire built-in session because app operations are unavailable. A manifest that
requires unavailable app capabilities remains unadmitted; this does not prevent
an unchanged Face coglet from running. Reconnecting at `0.2` requires fresh valid
session grants and normal ownership/epoch fences, not resurrection of old grants.

Required successor proofs (names reserved here, NOT tests executed by this
docs-only clarification): `local_contract_profile_defaults_and_unknowns`,
`local_contract_profile_manifest_format_independent`,
`local_contract_profile_catalog_v1_closed`,
`local_contract_profile_replay_pair_fenced`, and
`local_contract_profile_downgrade_face_speech`. Exercise each implemented consumer,
including omitted/default and explicit opt-in, catalog-free startup, conflicting
selectors, same digest/different profile reopen, legacy profile-less WAL, frozen
`0.1` negative names and live Face/speech-only downgrade. Add positive `0.2` app
controls separately. These complement, not replace, the native/transport proofs
below and the unchanged existing `0.1` suites.

### 2. Finite operation inventory

Every successful action returns the closed empty map `{}`. Failure uses the
existing common error and terminal lifecycle. Read arguments and successful
outputs are selected by the retained original operation, not a root union.
All operations are single-owner bindings to the supervisor-selected coglet.
No operation adds a synchronous guest read import.

| Operation | Direction | Closed arguments / payload / output summary |
| --- | --- | --- |
| `softmax.app.state` | read | `{}` -> safe app snapshot: windows/routes, focus, settings, PTT/HUD, hub status, Face state, revision |
| `softmax.app.controls` | read | `{window, after, limit, revision}` -> bounded semantic tree page and next cursor |
| `softmax.app.view_changed` | event | `{revision, window, route}` |
| `softmax.app.focus_changed` | event | `{revision, window, control}`; nullable focus |
| `softmax.app.control_changed` | event | `{revision, window, control, value}`; redacted value for protected controls |
| `softmax.app.window_changed` | event | `{revision, window, open, route}`; nullable route when closed |
| `softmax.app.navigate` | action | `{window, route}` |
| `softmax.app.open_window` | action | `{window}` |
| `softmax.app.close_window` | action | `{window}` |
| `softmax.app.set_focus` | action | `{window, control}` |
| `softmax.app.invoke` | action | discriminated closed union: press/toggle/select/set_value/type_text with window, control and verb-specific value |
| `softmax.app.set_setting` | action | closed discriminated union of setting key and its exact value type |
| `softmax.app.invoke_sensitive` | action | same verb contract, but only protected controls and the separate sensitive binding |

Ordinary operations belong only to `softmax.app`, with the exclusive resource
`softmax.app_control` required for actions. `invoke_sensitive` belongs only to
`softmax.app_sensitive`, requiring its distinct exclusive resource
`softmax.app_sensitive_control`. Both resources are two-component names. Sharing
a window or control id does not confer authority. The ordinary entry advertises
no sensitive action; the sensitive entry advertises no ordinary action. Having
only one resource must not create an alternate route to the other's effects.

The snapshot uses CLOSED records, not arbitrary UserDefaults, reflected model
objects, NSError descriptions or Keychain dumps. Windows have stable ids,
open/closed state and finite route ids. Inventory includes main (when present),
menu bar extra, Settings (all tabs), onboarding (all steps), Face, chat, orb,
HUD, command palette, About and app-owned sheets/dialogs. An unavailable optional
panel is reported closed, never fabricated open. Unsupported window/route pairs
are rejected. Native title-bar/menu controls and orb gestures need explicit
semantic registrations as well as SwiftUI controls.

Settings keys are an explicit typed allowlist derived from real UI settings:
appearance, text-size choice, floating-orb visibility, talk mode, sound cues,
hotkey configuration and launch-at-login state. Include any additional UI setting
only with its closed key/value variant and registration. Text size uses a finite
choice, not native floating-point storage. Hub state is a closed status enum,
not an invitation, identity record or arbitrary diagnostic string. PTT/HUD expose
state, visibility, integer elapsed milliseconds and bounded integer level, not
raw audio/transcripts. Face exposes visibility, mode and expression/status, not
private provider/session material. Validate nonfinite/out-of-range native values
before projection; the CBOR profile remains float-free.

Each control record has `{id, parent, window, role, label, value, enabled,
visible, sensitive}`. Parent and focus ids may be null. Roles and verbs are finite
sets. Values form a closed discriminator-bearing record union (none, boolean,
integer, bounded text, finite selection, redacted), not CBOR major-6 tags; secure values are always redacted, including empty
values, with no length, selection range or last-character information. IDs are
stable semantic identifiers, never view addresses, labels, random UUIDs, secret
values or list indices. Dynamic rows use safe product-owned stable keys.
Duplicates and dangling/cyclic parents fail registration. Reusing an id for a
new mounted instance must not authorize an action queued for the old instance.

Schema limits: ids/cursors at most 128 UTF-8 bytes, labels at most 256, ordinary
text at most 4096; page limit 1..128, maximum 4096 registered controls, maximum
64 windows/panels, maximum tree depth 32. Apply encoded-byte limits in addition
to schema bounds. The entire event/read wrapper must fit the existing transport
and extension limits; overflow is `resource_exhausted`, never silent truncation.
`revision` is a monotonic uint64 within a durable adapter source incarnation;
exhaustion fences the adapter. First tree page uses `after=null` and
`revision=null`; its reply fixes the revision. Later pages require that revision
and an opaque snapshot-bound cursor; mutation makes them `conflict`, never a
mixed-revision tree. Full settings/window inventories are bounded and atomic in
`state`; control pagination is not an atomic cross-operation snapshot.

### 3. Observations, events and privacy

The four listed events are new, explicitly bound app guest observations. They
are emitted by the registered adapter only after a real committed UI change,
under the event's current owner/grant/epoch, and journaled before guest delivery.
They are not read replies disguised as events and do not carry arbitrary state
snapshots. Reads remain runtime/supervisor-only. A guest can subscribe to these
four events and act, but cannot synchronously query the control tree or obtain a
new full-state guest observation through an unspecified inbox event. A future
full snapshot guest observation requires its own contract decision.

Never observe Keychain identity material, authentication/invitation/provider
tokens, secure field contents, or secret-derived labels/errors/settings.
Redaction happens before snapshot creation, event queues, diagnostics, encoding
or persistence, not as a final rendering filter. Public connection status is
allowed; exporting the identity store is not. Secret input action bytes, if
supplied by an authorized caller, remain private action/audit data under existing
retention policy and must not be echoed into readbacks, change events or errors.
Do not claim that observation redaction erases the original action journal.

The Swift SDK's current `openNativeObservationSession` stops the owner's backend
and drops that owner's frames; it must not be reused as an app-event registration
beside speech. Add a distinct, operation-scoped observation channel token for
trusted extension events. Bind it to session, owner, epoch, generation and an
explicit immutable event-name subset. Closing/replacing it invalidates only its
own extension publications, not speech's token, queued speech frames or native
backend. Global kill, owner revocation, disconnect and session teardown invalidate
both channels. Preserve one durable per-source sequence allocator across channels.
Test alternating speech start/end and app view/focus changes, token replacement
and partial writes to prove that neither channel steals the other's custody.
This generic SDK seam is a required implementation change, not an existing API.

Native app mutation owns revision assignment; capture safe immutable snapshots
on MainActor and pass bounded copies off actor. Never synchronously pump Wasm,
perform ledger I/O or block MainActor waiting on the runtime. Retain source
sequence continuity across reconnect/restart separately from UI revision. Queue
entries carry their originating owner, epoch, generation and mount incarnation;
recheck before encoding/publication and purge on revocation, disconnect or kill.
A stale callback must never borrow a successor's grants. Preserve existing SDK
private-output fencing including partially written frames.

### 4. Shared action path and sensitive controls

A registration modifier such as `.coglet(id:)` associates each interactive view
with its actual binding/command and accessibility identifier. AppKit panels,
menus and gestures register through the same semantic registry. Move local
routing/focus into app-owned state only where needed; do not maintain a second
shadow navigation model. Registry lifetime follows mounted instances, including
hidden/disabled controls and transient sheets. Every UI command, keyboard
shortcut, command-palette row, setting setter and adapter action calls the same
native command or binding setter, never a synthetic private actuator.

Dispatch resolves the exact currently mounted control and checks supported
verb, visible/enabled state, current route, owner/epoch, resource, deadline and
sensitivity immediately before entering that shared native command. Hidden,
disabled or vanished controls do not execute. `set_focus` uses the real focus
binding/responder; unsupported focus fails rather than recording fake focus.
`set_setting` delegates to the same registered setter and cannot write arbitrary
preferences or bypass protected settings. Return success only once the native
command/setter has applied, not merely when a task is enqueued. For asynchronous
commands retain pending admission and report its actual terminal disposition;
no synthesized success on restart or uncertainty. A modal confirmation is its
own real control; opening it is not confirmation of its destructive action.

Mark as `sensitive` every control/alias that resets or deletes hub identity,
unenrolls/signs out, quits, installs an update, or initiates/grants macOS
permissions. Permission-request buttons, privacy/settings deep links and login
approval paths are conservatively sensitive; opening macOS UI never grants the
OS permission itself. Pairing/enrollment confirmation is sensitive because it
changes identity authority. The same classification follows command-palette,
menu, keyboard and setting aliases, including confirmation buttons. Ordinary
navigation to Settings/privacy remains ordinary; it cannot press those controls.

`invoke` unconditionally refuses a sensitive target with `permission_denied`,
even if the caller happens to hold the sensitive grant; callers must use
`invoke_sensitive`. Conversely `invoke_sensitive` refuses ordinary targets.
No generic setting, navigate/open/close, toggle, type-text, shortcut or stale id
may trigger a sensitive effect through the ordinary binding. Secret text fields
may accept typed input through their registered path but never expose it;
identity-mutating submission still requires the sensitive operation. Do not
simulate keyboard chords or control other applications. Adapter grants do not
bypass macOS consent or app-native confirmations.

Keep SDK durable action admission, exact-envelope retry, expiry and independent
kill behavior. UI work posted to MainActor is a cancellable generation-bound
request; killing/disconnecting must fence its native effect without waiting for
a blocked runtime/control queue. Retry of a retained action cannot press again.
Quit/update may terminate the process: retain admission before the effect and
reconcile uncertain outcomes, never manufacture a terminal success after death.

## Alternatives

- Accessibility-only scraping and synthetic keystrokes: rejected as the
  production adapter. Accessibility remains an independent coverage oracle;
  scraping alone gives unstable ids, secret leakage and a second effect path.
- Separate implementations of each action for coglets: rejected; drift from
  user behavior and inconsistent permissions/validation.
- Flatten operation names to unrelated two-component aliases: rejected; loses
  the requested `softmax.app.*` catalog namespace. Make the narrow grammar
  amendment explicit and prove compatibility instead.
- Unrestricted key/value settings, generic RPC or reflection: rejected; grants
  would authorize unknown future effects and unbounded secret-bearing data.
- A universal full-state guest event or new read import: rejected in this scope;
  keep finite events and the existing recorded guest ABI.
- Sensitive boolean on an ordinary action only: rejected; a payload flag is not
  separate binding authority. Use a distinct catalog entry, operation and lease.

## Consequences and rollout

This PR is docs-only. Independent exact-head review and merge-queue landing of
this ADR are prerequisites to any implementation. Approval accepts the design,
not native/macOS, real guest or transport qualification. Record material review
changes on the owner card. The subsequent implementation PR must include the
naming amendment, product capability spec/catalog/schema sources, regenerated
consumers, fixtures and conformance cases as one coherent contract change.

Coordinate minimal shared codegen/workflow edits with their owners under Ground
rule 7. Preserve frozen catalog bytes, existing speech behavior and OS consent.
Integrate the app catalog into the installed runtime configuration/operation
allowlist; a fake registry beside the real host is not delivery. Rollback closes
new app bindings and queued effects while leaving native user actions usable,
speech independently scoped, and all historical receipts/catalog pins intact.

## Verification

These names are REQUIRED implementation proofs, not tests claimed to exist or
pass in this docs-only PR:

1. `softmax_app_catalog_coverage`: native UI test enumerates the independent
   accessibility hierarchy for every route, window, Settings tab, onboarding
   step, menu, palette, dialog and dynamic state. Every actionable accessibility
   node must map to exactly one registered stable semantic id and correct verbs;
   compare actual id sets, not just counts. Cover AppKit title-bar controls,
   expanded disclosures, orb gestures and keyboard/focus paths. OS-owned dialogs
   are outside the tree and are explicitly identified, not broadly excluded.
   Include a deliberately unregistered control negative control. Traversing only
   already-registered controls or one happy-path screen cannot pass coverage.
2. `softmax_app_observe_and_control`: launch the real app adapter through the
   real Swift SDK transport against coglet-mock with the pinned catalog and real
   grants. Navigate to Settings, receive/journal `view_changed`, toggle a
   setting, read it through the runtime read path and independently compare the
   rendered UI, set actual focus and press a real registered button. Verify
   outcomes, shared native callback counts, source/revision and preserved retry
   dedup; no mocked guest-script action handler replacing the app adapter.
3. `softmax_app_sensitive_controls_refused_without_capability`: ordinary grants
   plus each protected control and every alias/verb/setting/navigation bypass
   yield no native effect; test distinct sensitive operation/resource omissions,
   revocation after queueing and positive separately granted native confirmation.
4. `softmax_app_secrets_never_observable`: seed distinct identity/token/secure
   field canaries; inspect decoded reads, event bytes, partial queued output,
   errors and diagnostics before/after edits, focus, reconnect and revocation.
   Require constant redacted representations, including empty fields and labels,
   with positive nonsecret value controls. Keep original private action/audit
   inputs separate from the no-observation assertion.
5. `softmax_app_catalog_conformance`: language-neutral cases under
   `conformance/` drive both the app adapter and its mock through actual wire
   transports with the same pinned schemas. Cover exact entry/operation/resource
   advertisement, all operation directions, closed unions, invalid siblings,
   page bounds/stale cursors, secret/sensitive negatives, two/three-component
   naming boundaries, owner/epoch replacement, deadlines, kill, restart and
   identical retry receipts. Prove sensitive negatives reach an advertised and
   granted ordinary action rather than vacuously rejecting an unknown name.

Required compatibility proofs additionally run existing `conformance/surface`
and `conformance/speech` cases unchanged at `0.1`; add `0.2` cases separately,
never rewrite a frozen `0.1` case to test the new profile. Cover a new app-capable
peer negotiating down to `0.1` with live Face/speech and no app operations, an
old-only peer, and positive `0.2` app plus Face/speech coexistence. Compile and
exercise existing built-in-only Swift `SurfaceSession` call sites unchanged,
including omitted catalog input; separately test optional pinned extension
provisioning. Independent exact-head ADR review must explicitly cover both the
Face/speech `0.1` preservation and the optional generic Swift SDK API contract.

Required CI must execute schema/CDDL differential fixtures, regeneration drift
checks, all affected SDK/compiler regressions, native Swift compile/unit/UI and
shared conformance cases. Update only minimal existing workflow/registry seams;
retain the exact test identities and real executed head in artifacts. A skip,
source scan or Linux-only mock run does not qualify native app coverage. A test
that fails required CI then passes on rerun follows the
[quarantine rule](../ci-quarantine.md), with an owner card, rather than repeated
required stress runs. The implementation PR needs independent exact-head review,
green required CI and merge-queue landing before the task is done.

## Approval

Pending independent review and accepting merge-queue PR. No implementation or
qualification is authorized by this proposed document alone.
