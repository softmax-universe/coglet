# Existing-image transition increment (not final stage2 PASS)

Baseline at38c3ea12:124fast tests actually PASS. Retain all existing modules and
original95files/453definitions. Add image verifier tests first; separate Rust
resolver-selection tests before executable integration. No frozen contract change.

Independent initial review sa-0-70741bae CHANGES: real resolver consumption,
hidden warm requests, structural base-only invariance and honest sandbox limits.

Revised bounded plan:
1. Synthetic-only additive bridge command uses ONE actual CaosClient for all
   requests; same immutable resolver/tree, program, genuine recorded state and
   salt across A-cold/B-first/B-warm/A-return. Never construct a second runner.
2. Existing Unforged ResolverConfig::load + resolve_driver_name select actual
   tree-strategy executable from pinned operator-created resolver Git tree.
   Read selected resolver blob via existing GitEngine; require its exact bytes
   equal pinned native-only proof driver. Include entire actual resolver tree in
   args/resolver and execute selected path there, not unused digest metadata.
   Strategy has deterministic=true, driver=proof, no caos_base override. Resolver
   selection is existing API, not a new parser/scheduler. Changed/missing driver,
   wrong selection, extra executable and base override refuse before credentials.
3. One identical bash/Go polyglot worker1 delegates to same retained shell launcher
   (both images have bash per their checked-in flakes), materializes complete state
   and resolver via existing caos get -r, runs resolver-selected static driver in
   disposable workdir, publishes whole ABI tree /cas/out and sidecar separately.
   If any required interpreter/tool is absent, refuse; no fallback pretending a
   strict sandbox. This image-control increment is BOUNDED source-audited native
   replay, NOT strict namespace/no-live-effects qualification. Existing strict
   replay/fork/factory evidence remains distinct and unchanged.
4. Direct existing CaosClient request_with_headers sends exact /submit and /run
   requests; no run_arg_tree_detailed hidden status/HEAD probes. A-cold may use
   nocache=1; B-first MUST ordinary /run and observed miss; B-warm and A-return
   ordinary /run and explicit hit. Require exact X-Caos-Image for every invocation,
   missing/ambiguous header refusal. No inferred default cache classifications.
5. Complete args/bundle/salt are identical across every call, NO native-image or
   native-pool arg. Change ONLY Docker base immutable ref. Require returned A
   request equals Areturn and Bfirst equals Bwarm, A != B. Read raw submitted
   request objects and verify base/image and unchanged args. Retain authenticated
   raw statuses/sidecars and complete output object graph for all observations;
   compare every full ABI tree to genuine native oracle.
6. All inputs here are explicitly synthetic public fixtures, as prior resolver
   controls; NEVER call this new command for private inputs. Existing held-source
   and strong per-crossing private-grant implementation remains unchanged and
   separately exercised. No claim this synthetic control adds private transport
   qualification. No new caches, stores, scheduler, deployment, capacity or effects.
7. Add independent Python verifier: exact4ordered observations, same program/input/
   resolver/args/bundle, A/B request binding, exact image/cache headers, all whole
   trees. Missing/ambiguous receipts, changed args/resolver/image, Bfirst hit,
   Bwarm miss, Areturn B request and projected tree each fail closed. Actual
   hosted proof only after independent plan/code review and rebuilt latestpin.

Final seven-case composition still cannot claim PASS until integrated image
control, exact-head review/CI/nativequeue/main and containing-main proof complete.
