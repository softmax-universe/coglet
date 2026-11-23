# Spec-v0 cross-spec review ledger

Review baseline: `7cc8a9cf9b53a7ebcb452bb1ae0baffcadb624b9`.
Scope: conventions, ABI/journal/manifest, surface, speech/expression, hub,
engine findings, harness and codegen. Policy/promotion/tickets are excluded.
This is an open repair ledger, not a passing freeze report.

## Independently read back by the freeze owner

| Finding | Evidence at baseline | Required disposition |
|---|---|---|
| Host-import contradiction | ADR-0003 R4 lines 60–66 permits WASI only and argument/file-only results; ABI lines 28–45 specifies coglet_v0 imports, lines 217–225 adds grant/dedup host state | Amend ADR-0003 allowlist and recorded-input contract, add regression |
| Manifest resource contradiction | ABI line 282 requires exclusive_outputs subset of required_capabilities; surface lines 29–32 separates capability entries/operations/resources and lines 75–84 grants operations and resources independently | Define manifest operation/resource semantics and valid Face manifest, test admission |
| Relay payload/sequence contradiction | Hub lines 324–331 re-sources surface events, adds provenance inside payload and demands gap-free seq; ABI lines 73–77 consumes shared coglet seq for local wakes | Preserve capability payload validity, ownership and local/relay sequence semantics with cases |
| Budget under-specification | ABI lines 23–26 shares initialization/allocation budget; lines 151–181 leaves accounting TBD; ADR-0001 caps calls separately and platform ADR requires positive signed-int FFI bounds | Define cumulative qualified mode without claiming portable fuel |
| Guest receipt gap | ABI lines 122–123 promises later receipt event; surface lines 104–107 rejects without outcome; ABI has no named receipt payload | Define deterministic delivery and subscription rules, test rejection as well as accepted outcome |
| Engine evidence is not product acceptance | ADR-0001 microbenchmark caveats; platform ADR QEMU-only and unexecuted iOS SDK boundary; ADR-0003 lines 83–84 requires transform qualification before acceptance | Explicit bounded decisions, retain unperformed qualification gates |

## Additional review findings to adjudicate

Three independent read-only review sessions inspected the source, schema,
fixture and harness joins. Their findings below are leads, not claims that the
freeze owner has executed all reproductions:

- ABI trace shape requires nonempty step records/nonempty event batches, whereas
  hub requires durable stopped/paused boundary mutations without guest execution.
  Specify trailing/standalone intervention export completeness without fake steps.
- Hub executable provenance requirement is absent from ABI trace validator:
  add present/missing/mismatched provenance cases with recomputed hashes.
- Bind each speech/expression operation to its exact argument/result/event rule;
  a root union cannot enforce kind-specific payloads. Clarify trusted catalog.
- Read args for speech.config, speech.work_state, expression.state and success
  result maps need explicit shapes. Clarify event-only guest versus runtime reads.
- Clarify action.outcome routing, correlation and subscription bypass without
  delivering duplicates through surface events and inbox.
- Record one clock-translated action at dispatch and reuse identical bytes for
  retries; distinguish guest logical deadline from surface clock domain.
- Reserve runtime-owned event kinds against external self-authorization, not just
  runtime.debug_step. Keep private speech events owner-only through relay.
- Define llm.complete's gateway capability and upstream dispatch boundary, or
  explicitly narrow the current contract instead of implying a missing hub RPC.
- Define test-only sensor.tick, motor.move, other.write, hub.note/test.* examples;
  replace stale speech.speak with speech.speak_chunk where appropriate.
- Reconcile filesystem/debug paths (C0/DEL/backslash differences) with negatives.
- Replace stale spec/messages/README.md scaffold text.
- Extend behavioral data for speech.backchannel, session_open/session_close,
  tool_call/tool_result, expression.state, chunk bounds, idle/deadline stop,
  expression reset on kill and capability/resource admission.

## Infrastructure inventory (reviewer-reported)

`codegen/contracts.json` classifies all six current families as generated.
Current codec corpus: 245 positive shape cases, 445 negative shape cases,
17 wire negatives; seven semantic vector files are intentionally excluded from
codec crawling. Current protocol case files: ABI 24, surface 11, speech 8,
hub 52; engine remains a reserved scaffold. These counts do not establish runtime
conformance or exhaustive generated typed-API coverage.

Reviewer execution: 27 harness regression tests, nine registry tests and two
IR tests passed; 690 JSON Schema shape fixtures agreed; four registered loopback
self-tests passed. `codegen/run --check` failed to start its formatter because
`rustfmt` was missing on this worker, so freshness is not established locally.
No protocol implementation is registered in `conformance/impls.toml`.

## Repair batch 1 (PR #36, not freeze acceptance)

- Corrected ABI manifest semantics: operation grants and resource leases are
  independent namespaces; entry names are not operations. Eight source vectors
  in `spec/fixtures/abi/manifest-admission.fixture` validate through both manifest
  schemas and the semantic oracle. Generated `abi.manifest_admit` timelines run
  through the real harness with independently computed responses and flipped
  admission negatives (`test_manifest_timeline`). No runtime adapter is claimed.
- Amended ADR-0003 R4 to include the four ABI imports and the recorded grants,
  budgets and effect ledger. It remains Proposed: the spike linter is explicitly
  not the new ABI admission implementation, and transform qualification remains
  unperformed. Existing rehashed trace tests cover host-call precedence/ledger.
- Renumbered platform ADR 0002 to 0008, fixed design/site/spike references and
  plan/site freeze references, and indexed all current ADRs (0005 reserved).
  Status decisions are still pending, not silently changed by the index.
- Owner ran all 62 then-existing spec tests successfully with Python 3.12,
  pycddl 0.6.4 and reference Ruby cddl 0.12.14; after adding timeline data, the
  11 ABI tests and the new timeline test passed again. Codegen `--check` reports
  all 396 rules / five language outputs current. 27 harness regressions and
  four loopback self-tests passed. Full final-head checks/review remain required.

## Repair batch 2 (PR #36, not freeze acceptance)

- Resolved the path-control question: ABI filesystem components reject every C0
  control, DEL and backslash; hub debug paths remain a bounded ASCII subset.
  Unicode filesystem names remain byte-preserving. Changed generator sources,
  CDDL/JSON rules, semantic checker and all five generated consumers. Added
  positive ASCII/Unicode goldens and negatives for every forbidden character in
  both selected path and enclosing boundary-change rules.
- `test_path_contract` checks both actual schema validators for ABI/hub paths and
  rehashed snapshot negatives (so a hash mismatch cannot mask path validation).
  Owner ran 66 spec tests successfully before adding the corpus vectors; then
  all 11 ABI tests and three path tests passed after regeneration (104 ABI shape
  cases). Codegen freshness, 27 harness regressions and four loopback self-tests
  passed. Final full-head tests remain required.
- A further read-only design review recommends preserving relayed source/event
  bytes and introducing a separate relay offset (source seq has legitimate gaps),
  with private speech ownership keyed by target. This is a design lead, NOT an
  implemented or accepted fix. Guest receipts also need pending-action header
  state to verify exactly one terminal rejection/outcome across trace boundaries.

## Repair batch 3 (PR #36, not freeze acceptance)

- Hub relay now preserves the original common envelope/closed payload. Device
  assertions are admission metadata, not surface key proof. Local runtime wakes
  and foreign coglet sources are refused. Speech owner-only visibility is stated
  explicitly through relay.
- Added `hub-events-params.offset` and renamed result `next_seq` to `next_offset`:
  durable relay positions are contiguous while source sequences may have gaps.
  Retained batch receipts dedupe retries before checking source watermarks; empty
  batches only probe the current cursor and reserve no position. These changes
  are pre-freeze 0.1 repairs, not post-freeze minor-version compatibility claims.
- Updated source generators, both schemas, fixtures, all five generated targets
  and hub case data. Added seven relay cases (source gaps/repeats, conflicting
  offsets, local wakes, foreign/forged coglet sources, empty cursor probe).
  Generator allocation shifts subsequent correlation IDs but not their meanings.
- `test_hub_relay` executes all event timelines against an independent test oracle
  via the actual harness, including corrupted-response controls, exact payload
  preservation and retry-no-redelivery. This is case-contract testing, NOT a hub
  implementation; target grants/private-owner authorization and synthesized
  action-outcome authority still need dedicated behavioral cases before freeze.
- Owner ran 11 hub/relay tests, then the full 68-test spec suite successfully;
  codegen freshness and whitespace checks passed. Final-head independent review,
  CI, merge and tag are still pending.

## Repair batch 4 (PR #36, not freeze acceptance)

- Bound ABI replay to the existing hub `hub-build-record`: every snapshot retains
  canonical `/code/build/provenance.cbor`, and its rewritten digest must match the
  pinned executable/blob. Reused the hub schema rather than inventing a second
  provenance record. Signing and transform qualification remain admission gates,
  not properties inferred from matching hashes. Fixture identity transforms are
  expressly test-only.
- Regenerated ABI fixture trees and record chains with retained provenance.
  `test_provenance_trace` validates eight shape-valid, rehashed trace vectors
  (positive, missing, wrong output, missing policy, extra field, malformed CBOR,
  trailing CBOR and noncanonical CBOR), checks the shared CDDL rule and executes
  the corresponding harness timelines with flipped-result negative controls.
  No real replay implementation is claimed.
- Read failed Go/conformance CI logs for batch 3: Go RE2 rejects `\\u` regex
  escapes. Changed the CDDL spelling of ASCII control ranges to portable `\\xHH`,
  preserving JSON Schema semantics/JSON fixture escapes, then regenerated all
  five consumers. This matches the Core lane's explicit decision on the card.
- Owner ran all 69 spec tests, codegen freshness (396 rules/five outputs), Go
  module tests and all Go emitter tests successfully. Earlier this run 27 harness
  regressions and four loopback self-tests also passed. Final-head CI and
  independent review remain outstanding; no tag is authorized yet.

## Repair milestone M-A (PR #36, not freeze acceptance)

- ADR-0009 explicitly amends ADR-0006/#8's step-only trace representation.
  Traces now pin initial_files/code/manifest, final_hash and nullable separately
  hashed trailing_boundary. Zero-step standalone exports are valid; boundary
  records contain no fake event/step/engine usage. Mount/inbox/log rules, exact
  mutations, provenance, chain identity and final-tree binding remain checked.
  Sealed trailing records end a segment; continuation carries derived pending
  state into a new trace without rewriting history.
- Guest terminals use only durable inbox materialization: runtime.action_rejected
  imports common action-accept constrained to accepted=false; action.outcome uses
  common action-outcome. Runtime.receipt is structural runtime-origin audit, not
  a hub RPC. Complete initial_actions pending arguments join the lifetime ID
  ledger. Terminal append consumes pending state atomically; its wake may remain
  queued, fail durably, or cross a trace boundary without duplicate terminals.
- Added build_boundary_receipts.py and 46 regenerated, shape-valid/rehashed
  boundary-receipts.fixture vectors plus executable conformance/abi case contracts.
  test_boundary_receipts checks both actual schemas, recomputed hashes, pending
  state with a separate terminal-ID projection, real harness matching, and corrupt
  valid/dispatched/engine_invocations responses. Includes standalone/trailing and
  ordered/delete writes, wrong chain/time/final/hidden mutations, terminal rejection
  and all outcome statuses, initial pending state, post-commit/pre-step delivery,
  aborts, trace-crossing wakes, unknown IDs/correlation/source/origin and duplicate
  delivery with fresh inbox IDs/source seq. This is archive/case-contract testing,
  NOT engine/runtime conformance; engine_invocations=0 means validation only.
- Regenerated both ABI schemas, legacy fixture chains and all five consumers.
  Local verification: pytest spec/tests: 72 passed; codegen --check: 400 rules,
  all five targets current; cross_check: 765 fixtures agree across independent
  Ruby CDDL/JSON/generated validator; 27 harness regressions and four loopback
  self-tests pass. Exact-head hosted CI is recorded in the card handoff.
- Read-only review found stale generated vectors during development (regenerated,
  generator-equality/full-suite checks now pass) and asked to distinguish source
  binding from authority. ABI now explicitly states the archive authority gate.

### Remaining rows for M-B / final freeze

M-A closes the journal boundary export and guest terminal/pending-action format
rows, not the whole ledger. Existing repairs close manifest/path/provenance and
relay payload/offset representation. M-B still owns:

1. Cumulative alloc/handle budgets, bounded ADR-0001/0008/0003 decisions and stale
   TBD/design/scaffold wording, with explicit unperformed engine qualification.
2. Exact capability argument/read/result bindings; immutable translated dispatch
   envelope/retry clock mapping; llm.complete upstream boundary; reserved runtime
   and test capability names.
3. Speech behavioral gaps and relay grant/private-owner/synthesized-outcome
   authority cases. M-A's source checks and synthetic-outcome vector assume the
   separate retained authority audit; they do not prove authorization.
4. Adjudicate every residual ledger row with a tested spec disposition or explicit
   noncontradictory M1 qualification gate naming its Phase 2 owner.

The freeze owner retains independent exact-head integration review, all required
CI, merge/postmerge verification, spec-v0 tag and final inventory. No M-A merge,
tag, real engine execution, or production qualification is claimed.

## Remaining freeze work

1. Publish/link ADR-0009 proposal before changing normative specs.
2. Reproduce/adjudicate remaining findings; repair the actual contradictions and
   add focused shape/semantic/behavioral cases without inventing runtime success.
3. Decide ADRs, rename platform ADR to 0008, repair links and index all ADRs.
4. Run full required pinned-environment checks, regenerate all selected consumers,
   and obtain independent exact-head PR review.
5. Merge with green CI, verify landed checks, create/read back `spec-v0` at the
   merge commit, comment the final inventory/SHA/M1 deferrals on the task and
   request same-card review. Until then the serial gate remains incomplete.
