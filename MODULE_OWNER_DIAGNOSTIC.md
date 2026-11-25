# Non-delivery diagnostic: serialized native module owner

Goal: measure removing repeated WAMR load while preserving a fresh mutable instance per native handle. This is NOT approved implementation, qualification, or product delivery.

Existing profiling: with stripped qualified guest + decoded exact code pin + borrowed tree hash, actual map completed 15 independent replay records with one threaded provider call, but worst tick 50,364us > unchanged 50,000us. Compiled profiling found native create 19–35ms; guest ~6ms; WASI commit ~2ms; record/hash/validation ~10ms. Host variability remains real, not a waiver.

Prototype invariants to test before any production extraction:
- Rust's existing process-global RUNTIME mutex serializes all owner/instance/teardown calls.
- One retained owner at most, keyed by exact ORIGINAL bytes, runtime_bytes and import allow_db. No hash-only identity.
- Owner retains mutated loader byte backing, pool, module, import registry and initialization. Never retain a module across wasm_runtime_destroy.
- Budget both original/mutable byte buffers inside the configured runtime_bytes total by subtracting them from pool capacity. Overflow/missing capacity fail closed. Linear pages/stack/dynamic compute/mode/deadline still checked on every instance/handle.
- Every call creates/destroys a NEW wasm instance and exec env; watchdog joined first. No instance/global/linear-memory reuse.
- Eviction on any owner-key difference destroys the previous module/runtime/pool/backing before constructing another; failure leaves no cache. Mixed ABI/import/module owners must invalidate.
- Tests must cover sequential globals/memory isolation, interleaved programs, policy/capacity/deadline/mode changes, bad module/instance failures and recovery, independent replay and exact wire/timing.
- Retention lifetime/API/accounting and engine build identity need independent architecture review before ANY production PR. Static process retention is a diagnostic convenience, not a settled SDK lifetime contract.


## Executed result (diagnostic only)

- `perf-integration-module-owner-compiled.log`: PASS unchanged50ms, worst25.358104ms,12pendingticks/14independent native replay records and repaired-hash tamper/foreign-code rejection.
- `perf-integration-module-owner-native.log`: PASS unchanged50,000us, worst40,531us on actual pinned map; Java130/codegen10,12real observations during threaded production Gateway pending provider, one ordinary research, all15independent replay/provider_calls1. Archive `.cache/avatar-proof` corresponds to this run until the next fixture invocation overwrites it.
- `module-owner-wamr-tests.log`: full release engine suite passes, including admission_cache fresh global/memory reset, changed static/dynamic policies, imports/NaNs/native qualification/deadline recovery/steps/WASI. Additional new-owner-specific failure/lifetime controls are STILL missing.
- The original non-owner detailed actual-map run remains RED50,364us and compiled RED62.000381ms. Do not infer production acceptance from the experimental owner.

## Recommended production lifetime shape (NOT implemented)

Replace process-static unlimited retention with one owner lease identity per immutable Module proof, shared by Module clones. Allocate a unique checked-generation ID at load; every handle passes that lease ID to the serialized native owner. The retained C owner keys by lease ID + exact original bytes + runtime_bytes + import policy; new identities evict old module/runtime/pool/backing before creation. A final Rust lease Drop acquires the SAME RUNTIME mutex and releases the C owner only if its identity matches, so dropping an old/interleaved proof cannot destroy another owner's cache. Never acquire this lock while already executing/dropping a native instance; inspect destructor ordering on success, refusal and unwind. WamrInstance consumes/clones its Module lease, guaranteeing it survives execution; all instance/watchdog destructors must finish before final lease release. Cache contains no guest instance/global/linear memory. Keep actual pool + both retained byte buffers within runtime_bytes. This shape still needs independent architecture validation, exact-head tests, CI and landing, and may need a different shape after review.

The default delegated reasoning route returned HTTP402 billing; no independent review was performed and no route was bypassed. The prototype must NOT be opened/merged as a production PR.
