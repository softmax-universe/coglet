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
