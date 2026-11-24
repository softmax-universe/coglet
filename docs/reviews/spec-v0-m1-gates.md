# Spec-v0 residual ledger and M1 qualification inventory

Residual qualification owner after bounded ADR-0014 M1 acceptance:
**t_01a20c2f7b9bb95a**. Its [work-in-progress evidence map](m1-residual-qualification.md)
tracks transform publication, broader synthesis/private/relay matrices and retry
crash points. Local supporting tests do not close the B rows below; required-CI
qualification or an accepted ADR disposition remains necessary.

Scope owner/final acceptance: **t_01a205d727e02f52**. Serial M-B repair:
**t_01a2073e2797f99d**, on existing PR #36 only. This inventory is normative
qualification gating under ADR-0009, not permission to execute unqualified code.
A closed representation row does not qualify its real implementation. Every row
below has exactly one disposition: **A = specified and tested**, or **B = explicit
M1 qualification gate**. Subrows separate wire ambiguity from implementation
proof; they do not award both dispositions to the same claim.

No gate weakens an existing MUST, supplies credentials/authority, promises
exactly-once physical effects or declares an implementation registered. Until its
proof passes, refuse the corresponding mode/route/feature before execution or
advertisement. Do not silently substitute a mock, fixture oracle or identity
transform. Future contract changes still require an ADR. Required qualification
case names below are acceptance obligations for the named existing owner card,
NOT claims that those implementation tests already exist or passed.

## Disposition ledger

| ID / residual row | Disposition | Evidence or primary M1 owner | Required proof / frozen rule |
|---|---|---|---|
| E-budget: cumulative initialization + abi_alloc + handle accounting | **B** | engine-wamr **t_01a205e4fdee443e** | ABI Budgets and ADR-0009 specify ONE allowance and absolute deadline, exact remaining budget and positive signed-i32 FFI bounds. Require `cumulative_init_alloc_handle_budget`, `exhausted_alloc_never_calls_handle`, `allocation_does_not_reset_wall_deadline`, `ffi_budget_zero_overflow_rejected`, `host_stack_and_memory_bound`, plus aggregate usage/abort and fresh-instance recovery. Reject unqualified modes before instantiation; spike per-call setters do not prove this. |
| E-0001: bounded ADR-0001 evidence/production qualification | **B** | engine-wamr **t_01a205e4fdee443e** | Interpreter-first and engine/build-specific units only; AOT does not supply compute, ordinary AOT does not supply deadline. Require unsupported-mode rejection, initial AND maximum memory admission, growth/overflow controls and cancellable bounded imports. Microbenchmark timings are not product latency/capacity acceptance. ADR-0009 records this bounded decision; formal status stays with freeze owner. |
| E-0008: bounded platform evidence | **B** | engine-wamr **t_01a205e4fdee443e** | ARMv7 is QEMU evidence only; iOS archive/link/device remain unverified. Qualify each claimed target separately (`identified_eo1_compatible_sysroot_native_rss`, `ios_archive_link_signed_device`); unsupported/unverified targets are not runnable claims. M1 on a qualified Linux host does not require claiming EO1/iOS success. |
| E-0003: bounded determinism/allowlist qualification | **B** | engine-wamr **t_01a205e4fdee443e** | R1–R5 with amended R4 permits exactly the four ABI imports plus restricted WASI and recorded admission state; spike linter is not production admission. Require `production_import_allowlist`, `float_nan_cross_arch_replay`, no threads/shared memory/SIMD when forbidden and exact recorded-input replay. No cross-engine resource-cutoff equality or byte hash normalization. Formal policy status is freeze-owner work, not fabricated implementation proof. |
| H-transform: real publish-time rewrite and signed provenance | **B** | hub proto **t_01a205e73c70350a** | Require `rewrite_before_build_commit`, `unqualified_transform_publication_refused`, actual pinned-transform fuzz/cross-architecture evidence and authenticated publication/provenance admission. Existing identity-transform provenance fixtures only prove archive shape/hash binding. This supplies hub-side proof complementing E-0003, not an alternate owner of the same engine tests. |
| W-binding: exact operation, event, read args/results and action success result selection | **A** | `test_contract_bindings.ContractBindings.test_selected_binding_vectors_both_schemas`, `test_catalog_complete_and_exact_direction`, `test_generic_envelope_cannot_select_sibling_or_llm_action` | `spec/capabilities/bindings.json` selects both directions; imported CDDL/JSON rules, `surface:empty-map`, golden selected vectors, missing/unknown fields and valid-sibling negatives. No generic root union fallback. Successful action result is closed `{}`; config/work_state/expression.state read args are closed `{}`. |
| W-llm: llm.complete ABI/gateway binding | **A** | `test_contract_bindings.ContractBindings.test_llm_selected_capability_requires_exact_operation`, `test_catalog_complete_and_exact_direction`, `test_generic_envelope_cannot_select_sibling_or_llm_action` | Only built-in ABI llm.complete; llm-request/llm-reply, NOT surface act or undeclared hub RPC. Provider input/output remain profile maps. `bindings.md` defines retained identity/correlation, recorded inbox reply and adapter boundary; no synchronous guest read import. |
| L-gateway: actual LLM route/spend/receipt behavior | **B** | LLM **t_01a205e78cbb1f52** | Require `gateway_requester_grant_route_binding`, `reply_request_correlation_binding`, `uncertain_provider_delivery_reconciled`, `record_before_inbox`, replay with no live call and spend/timeout/cancel cases. No production remote route until an authenticated adapter is qualified; a new gateway wire protocol requires ADR, never an invented hub method. |
| R-retry: immutable clock-translated dispatch/retention | **B** | runtime core **t_01a205e5cb4956e5** | Surface clock section and ADR-0009 fix the semantics: journal immutable ABI intent, translate ONCE, persist complete act including grant epoch before send, replay IDENTICAL deterministic CBOR on retries. Require `clock_resample_retry_same_bytes`, `crash_after_persist_before_send`, `reconnect_reconciles_old_epoch`, `unknown_admission_no_new_id`, `terminal_rejection_allows_new_id`; do not extend deadlines or re-admit unknown outcomes. |
| W-reserved: reserved runtime/test names and production catalog exclusion | **A** | `test_contract_bindings.ContractBindings.test_reserved_and_illustrative_names_are_not_surface_bindings` and generic-envelope negative | `bindings.md` defines runtime.inbox/timer/debug_step/action_rejected, audit-only runtime.receipt, action.outcome and llm.reply authority boundaries; runtime.* cannot be a surface capability. sensor.tick/motor.move/other.write/hub.note/test.* are isolated fixture names, speech.speak is NOT a production alias. Passing name grammar is not admission. |
| R-origin: actual runtime wake/receipt authority enforcement | **B** | runtime core **t_01a205e5cb4956e5** | Require `external_runtime_wake_refused`, `runtime_receipt_not_rpc`, `terminal_append_pending_action_authority`, subscription-bypass/no-double-delivery and durable dedup after restart. M-A tests source/pending structural binding; an actor/source string is not authentication. |
| S-speech: remaining native speech/provider/expression behavior | **B** | surface Rust + mock **t_01a205e673b63cb9** | Before advertising affected operations require `backchannel_own_stream_and_one_terminal`, `session_open_ready_then_close_fences_callbacks`, `tool_call_owner_pending_once`, `tool_result_closed_unknown_conflict`, `chunk_64_utf8_bound_and_gap_cancel`, `idle_closes_stream`, `deadline_stops_audio`, `expression_state_snapshot_and_kill_reset`, duplex/resource grant admission, revoke/disconnect/kill and no successful synthesis-only completion. Run SAME applicable harness cases against real + mock adapter; native/acoustic latency needs measured device evidence and is not proven by scripts. Existing history/work fixtures remain required, not waived. |
| H-relay-grants: target/method grant enforcement | **B** | hub proto **t_01a205e73c70350a** | Require `relay_target_without_grant_denied`, `relay_wrong_actor_denied`, expired/revoked grant and cross-target replay controls against authenticated sessions. Existing relay offset/payload structural tests do not prove these. |
| H-private: speech private owner visibility | **B** | hub proto **t_01a205e73c70350a** | Require `relay_private_owner_only`, `relay_subscription_not_authority`, ownership transfer purges/fences old observations and private read/event leakage negatives. Transport enrollment/delegation alone cannot widen audience or local leases. |
| H-synthesis: synthesized outcome authority | **B** | hub proto **t_01a205e73c70350a** | Require `synthesized_success_without_authority_denied`, unknown/unadmitted action, wrong source/owner/correlation, duplicate terminal and retained authority audit controls; valid authenticated surface outcomes and authorized terminal failure controls must still work. Source syntax or test-only trace vectors cannot authorize success or a physical effect. |
| F-path: production path enforcement after portable-schema broadening | **B** | fs **t_01a205e58c02c6ae** | Preserve existing `test_path_contract` and forbidden-character fixtures; require runtime mount/descriptor tests for all C0, DEL, backslash, dot/empty segments, escapes and Unicode exact bytes. Codecs alone do not enforce the full path contract. |

### M-D carried qualification gates (non-blocking for v0)

These are disposition **B**, not unperformed v0 wire changes. Required proof
names below are unexecuted M1 obligations, not claims of native qualification.

| ID / residual row | Disposition | Named M1 owners | Required proof / frozen rule |
|---|---|---|---|
| R-heard-private: heard-path privacy and debug exclusions | **B** | runtime core **t_01a205e5cb4956e5** (primary) | Supporting owners: fs **t_01a205e58c02c6ae**, hub proto **t_01a205e73c70350a** for remote debug. Require `heard_path_guest_write_create_denied`, `heard_path_debug_write_denied`, `heard_archive_private_owner_only`, `heard_revocation_denies_live_not_rewrite_archive`. The reserved runtime-write-only path cannot be mutated through guest/debug/inbox routes; transcript privacy applies to archive/export/debug access. |
| R-heard-persist: heard persistence and native alignment | **B** | runtime core **t_01a205e5cb4956e5** (primary) | Supporting owners: fs **t_01a205e58c02c6ae**; Phase 2 surface Rust **t_01a205e673b63cb9**, Swift **t_01a205e6f7d2d882**. Core/fs require `heard_durable_before_reasoning`, `heard_crash_restart_archive_only_replay`, `heard_owner_reads_append_only`. Rust/Swift require `native_item_alignment_owner_bound`, `native_alignment_only_appends_spans`, `stopped_item_alignment_frozen`. Richer native alignment is NOT required for v0 and may only append spans consistently with ADR-0009 M-D, never rewrite archived spans. Contract-oracle vectors do not prove native alignment or durable storage. |
| H-sender-epoch: hub.actions sender epoch-map retention | **B** | hub kernel **t_01a205e767bb96f0** (primary) | Supporting owner: hub proto **t_01a205e73c70350a** for the transport/clock boundary. Require `hub_actions_sender_map_and_envelope_durable_before_send`, `hub_actions_restart_retry_identical_bytes`, `hub_actions_new_sample_does_not_remap_old_id`, `hub_actions_missing_stale_map_refused`. Establish mapping to the receiving runtime epoch and retain the complete once-mapped request before first send; retries cannot resample/retranslate. |

## Freeze-owner handoff and scope boundary

Post-freeze disposition (2026-11-23): David directed merge-as-is and fix-forward;
ADR-0009 is accepted and `spec-v0` remains at
`c9c22b6878898282848484c52df4d37c054f6720`. Post-freeze review and formal status
reconciliation of the other ADRs belong to **t_01a207e76001ac2c**. This includes
reviewing byte-count versus character-count enforcement for the speech read text
cap under S-speech; no native validation is implied by schema acceptance. The
freeze card owns only the separate rustfmt CI fix and this acceptance record.
The pre-freeze handoff below is retained as history, not a new hold on the tag.

ADR-0009 records the bounded decisions and amendments. M-B does not change the
formal Proposed status of ADRs 0001/0008/0003, accept unperformed qualification,
merge #36 or tag spec-v0. The freeze owner must decide formal statuses using the
bounded text, perform independent exact-head whole-spec integration review,
required CI/merge/postmerge verification and tag/inventory acceptance. These are
existing final freeze work, not M1 behavior claims or residual wire TBDs.

Earlier manifest/path/relay-representation/provenance/M-A boundary and terminal
rows retain their tests in `spec-v0-cross-spec.md`. Stale scaffold/placeholder and
per-call-reset wording is corrected in messages/README, ABI and design of record.
Policy/promotion/tickets remain excluded. No new PR, ADR number or owner card is
created by this inventory.

## M1 Linux route evidence (ADR-0014)

[tests/m1](../../tests/m1/README.md) defines the exact qualified route and named
integration tests. `PROVEN` below is bounded to the stated slice; `REFUSED` is
not a claim that the omitted positive qualification ran. Original A/B
obligations above remain intact. The merged Phase 2 PRs are the execution
receipts for their named tests. Alternate test names below are explicit
semantic mappings, not invented aliases or a claim every proposed name exists.

| Row | Evidence and qualification boundary |
|---|---|
| E-budget | **PROVEN Linux interpreter**, [#47](https://github.com/softmax-universe/coglet/pull/47): `cumulative_init_alloc_handle_budget`, `exhausted_alloc_never_calls_handle`, `allocation_does_not_reset_wall_deadline`, `ffi_budget_zero_overflow_rejected`, `host_stack_and_memory_bound`, `runaway_deadline_then_next_fresh_call_succeeds`. |
| E-0001 | **PROVEN interpreter admission / REFUSED AOT**, #47: `both_initial_and_declared_maximum_memory_are_admitted_before_execution`, `native_linear_memory_growth_limit_holds`, `widened_pointer_overflow_aborts_before_copying`, `unqualified_aot_mode_rejected_before_execution`. No product latency/capacity claim. |
| E-0008 | **REFUSED native EO1/signed iOS claims**. [#62](https://github.com/softmax-universe/coglet/pull/62) is archive/SDK-link only; ARMv7 remains QEMU. Linux M1 does not claim the two required device tests ran. |
| E-0003 | **PROVEN bounded import/replay slice**, #47: `production_admission_rejects_ambient_and_wrong_signature_imports`, `production_import_wiring_calls_all_four_allowed_functions`, `every_restricted_wasi_signature_is_wired`, `float_nan_cross_arch_replay`; M1 `replay_state_hash` independently executes compiled echo. Disabled threads/shared-memory/SIMD builds are not all-platform R1–R5 certification or cross-engine cutoff equality. |
| H-transform | **REFUSED publication qualification**, [#48](https://github.com/softmax-universe/coglet/pull/48) `validator_code_requires_valid_wasm_and_canonical_output_bound_provenance` is provenance binding, not real transform/fuzz qualification. M1 performs no publication; identity fixtures do not close this gate. |
| L-gateway | **PROVEN mock/offline only / REFUSED live provider qualification**, [#42](https://github.com/softmax-universe/coglet/pull/42): `recorded_reply_and_exact_inbox_replay_offline_after_reopen`, `invalid_requests_grants_routes_duplicates_and_receipts_fail_closed`, `uncertainty_survives_restart_no_resend_or_double_debit`, `definitive_timeout_and_cancellation_are_journaled_failures`. |
| R-retry | **PROVEN retained retry slice**, [#63](https://github.com/softmax-universe/coglet/pull/63): `ffi_byte_identical_retry_after_clock_advances_and_reconnect_no_duplicate_act` and socket counterpart; `durable_grant_epoch_survives_reconnect_and_restart`, `ambiguous_wal_failure_fences_native_clock_and_retained_dispatch`. [#50](https://github.com/softmax-universe/coglet/pull/50) `terminal_rejection_consumes_pending_action_once_and_keeps_lifetime_id` does not independently prove new-ID-after-terminal or every crash point. Those broader claims remain unqualified. |
| R-origin | **PROVEN source/recovery slice**, #50: `recovery_rejects_schema_valid_forged_runtime_wakes`, `uncommitted_timer_cannot_be_delivered`, `initial_inbox_wakes_are_seeded_and_missing_recovered_wakes_are_rejected`, `durable_terminal_append_uses_shared_source_sequence_and_survives_restart`. M1 `production_runtime_rejects_test_echo` and `fixture_catalog_requires_explicit_grant` preserve explicit fixture authority, never actor-string authentication. |
| S-speech | **PROVEN SDK/mock slice / REFUSED acoustic qualification**, [#49](https://github.com/softmax-universe/coglet/pull/49)/[#52](https://github.com/softmax-universe/coglet/pull/52): `chunk_gap_duplicate_and_stream_rules`, `chunk_success_only_comes_from_the_backend`, `accept_then_outcome_and_expression_readback`, `kill_is_local_latched_and_not_clearable_by_wire`, `unqualified_optional_operations_are_never_advertised`, `revoke_cancels_effect_and_fences_captured_native_epoch`, `kill_before_completion_and_deadline_at_completion_have_no_effect`. Withheld optional operations and physical latency remain unqualified. |
| H-relay-grants | **PROVEN authenticated registry/enrollment slice**, [#58](https://github.com/softmax-universe/coglet/pull/58): `registered_binding_exact_source_policy_and_revocation_fence_events`, `enrollment_grants_and_live_revocation_on_real_iroh`; #48 `grant_expiring_during_delegation_check_is_denied`. M1 `hello_coglet` exercises real invite/enrollment, health and revocation, not all-method/all-target certification. |
| H-private | **PROVEN registry privacy slice**, #58 `private_speech_owner_cannot_be_widened_by_another_registration`. Private read/ownership-transfer and remote-debug routes receive no additional qualification from M1. Enrollment/subscription never widens authority. |
| H-synthesis | **PROVEN unknown-effect refusal slice**, #58 `crashed_attempt_queries_original_scope_receipt_and_never_replays_unknown_effects`; unknown effects remain unavailable. The full synthesized-success authority matrix remains unqualified, not exercised or advertised by this M1 route. |
| F-path | **PROVEN real filesystem enforcement**, [#44](https://github.com/softmax-universe/coglet/pull/44): `every_frozen_path_forbidden_fixture_through_real_guest_and_boundary_apis`, `all_c0_del_backslash_escape_roots_and_dot_components_are_denied`, `no_unicode_normalization_and_directory_entries_are_utf8_sorted`; #50 `deserialization_cannot_bypass_snapshot_path_validation`, `filesystem_overlay_enforces_mounts_and_abort_isolation`. `test_path_contract` remains required. |
| R-heard-private | **PROVEN reserved-path/local-owner slice**, #44 `reserved_inbox_heard_events_reject_guest_create_write_rename_unlink`, `host_appends_preserve_exact_bytes_and_debug_cannot_change_code_or_logs`; #63 `ffi_revoked_heard_reply_never_enters_archive`. No remote-debug privacy widening. |
| R-heard-persist | **PROVEN local native/archive slice**, #63 `ffi_owner_heard_reads_materialize_before_reasoning`, `ffi_out_of_order_reads_do_not_rewrite_stopped_history_or_duplicate_observations`; #50 `heard_boundary_is_hash_chained_and_replayed_without_surface_access`, `heard_monotonicity_rejects_rewritten_alignment`; [#43](https://github.com/softmax-universe/coglet/pull/43) Swift `testNativeItemAlignmentOwnerBound`, `testNativeAlignmentOnlyAppendsSpans`, `testStoppedItemAlignmentFrozen` is SDK, not device evidence. |
| H-sender-epoch | **PROVEN immutable sender-map slice**, #58/[#66](https://github.com/softmax-universe/coglet/pull/66): `mapped_requests_are_immutable_across_restart_and_never_retranslated`, `mapping_absence_overflow_and_restart_never_create_or_remap_intent`. Missing mapping cannot authorize a fresh send; no guessed clock wire method. |

W-binding, W-llm and W-reserved retain their original **A** schema-test evidence.
This audit deliberately does not convert partial evidence into full B-gate
closure. Remaining positive obligations are unclaimed, and no withheld route is
authorized by the echo integration. In particular archive consistency is not
independent program replay; the latter is newly exercised by `replay_state_hash`.
