# LLM gateway

Trusted in-process outbox gateway for the frozen ABI `llm.complete` boundary in
`spec/capabilities/bindings.md`. This is **not** a new hub RPC or surface action.
Mock providers remain the default: opening a gateway performs no network I/O.
No legacy/product code was ported.

## Hub embedding

1. Run on a bounded **blocking** outbox worker, never inside guest `handle` or on
   an async reactor. Admit only committed runtime requests. Construct `Admission`
   from authenticated host identity and the current `llm.complete` grant.
2. Configure trusted logical-model `Route`s (provider identity and positive ceiling
   in integer micro-units of one currency). `set_budget` is per-coglet operator
   policy. Admission atomically reserves the ceiling; spent + reserved cannot
   exceed budget, even across connections. Budget reduction below exposure fails.
3. Construct `http::HttpProvider` for OpenAI Chat Completions or Anthropic Messages.
   It accepts exactly `input: {"prompt": "..."}`. The provider model, full endpoint,
   token/input limits, pricing, secret name and timeout are **host configuration**,
   never fields interpreted from guest input. No tools, streaming, arbitrary
   provider options, prompt caching or ancillary charge schemes are enabled.
4. Implement `http::SecretStore::get` with the hub's existing secret-store accessor
   (or supply a closure). Keys are fetched at dispatch, can rotate without restart,
   and are never stored in the gateway DB, reply or debug logs. The library does
   not select an OS secret store or read a global environment credential itself.
   Do not put keys in route names, URLs, fixtures or policy revisions.
5. Route `Call.provider` to that adapter using `dispatch_next_checked`:

   ```rust,ignore
   gateway.dispatch_next_checked(
       |call| host_authority.check_current_llm_grant(&call.coglet),
       |call| configured_providers.get_mut(&call.provider)
           .ok_or("unsupported provider".to_owned())?.complete(call),
   )?;
   ```

   Serialize authorization/revocation with the host's dispatch boundary; library
   possession is administrative authority, not authentication. The check runs on
   **cache hits too**. `dispatch_next` remains for hosts that already fence/check
   all queued identities externally (and the existing mock API). Failed checks
   conservatively leave an uncertain reservation without any dispatch.
6. Read `replay` after settlement; allocate a normal `llm.reply` inbox message with
   authenticated hub sender, global index/seq/ID and retained correlation. Call
   `record_inbox` with its exact canonical JSONL, then deliver those same bytes
   through ordinary authorized `hub.inbox`. Runtime owns dedup, boundary commit,
   `runtime.inbox` wake and trace export. Never mint a runtime event at the gateway.
   `replay_inbox` is the immutable delivery retry payload.

This card supplies reusable adapters and the gateway, not a deployment of the hub
or a new daemon. The embedding product supplies real authority, secret-store and
outbox/delivery plumbing; no claim of a live hub rollout or production credentials.

## Provider safety and accounting

- HTTPS is required, except **literal loopback HTTP** for a local fake/pool proxy.
  No URL credentials, queries or fragments, redirects, proxy auto-discovery or
  transport retries. Responses are bounded to 1 MiB and the configured deadline.
- Pricing is checked integer micro-units **per token**, rounding rates upward when
  necessary. `required_ceiling()` is `(max_prompt_bytes + 1024) * input_rate +
  max_output_tokens * output_rate`. Configure the Route ceiling at least this high.
  Returned usage settles actual token charges at those configured rates, not a
  provider-supplied price. Zero rates are allowed for a free/local provider.
- The supported deployment contract is a text model with at most one input token
  per UTF-8 prompt byte plus 1024 wrapper tokens, and no extra fees. Do not use
  this adapter for a model/pricing scheme without that verified bound. The gateway
  cannot constrain an external vendor that violates its token/billing contract.
  Excess/missing usage, malformed receipts and non-text/tool responses retain the
  **full reservation** for explicit reconciliation; they never silently debit zero.
  Ancillary Anthropic cache usage is refused rather than undercounted.
- Definitive HTTP rejections (400/413/422 invalid argument, 401/403 permission,
  404 not found, 429 exhausted) become sanitized failed ABI replies at zero charge.
  They assume the provider's normal unbilled-rejection contract. All other non-2xx
  statuses, transport loss/timeouts and ambiguous bodies remain uncertain. Error
  bodies, URLs, keys and raw transport error text are never copied into recordings.
- Claim is committed **before** I/O. A crash in that gap can leave an unsent call
  uncertain. There is no automatic retry. `reconcile` requires an actual definitive
  receipt, validates reply identity/schema and cost bounds, then atomically stores
  terminal bytes, debit, reservation release and successful cache entry. Repeated
  settlement cannot replace a recording or charge twice. This is not cross-service
  exactly-once delivery.

## Durable rate limits and response cache

`set_rate_limit(coglet, max_calls, window_ms)` installs a fixed admission window
starting at the first accepted request. `enqueue` uses host Unix milliseconds;
`enqueue_at` is the injectable trusted-clock variant, **not guest logical time**.
Counts include cached requests and are per coglet across all its routes. Denied
budget/duplicate/rate admissions do not consume a slot. Counts, start and last
accepted time survive restart. Backwards time fails closed. Reconfiguration does
not erase the window/count. The default is no rate policy.

Caching is **off by default**, explicitly enabled per coglet with
`set_cache_revision(coglet, Some("trusted-revision"))`. Bump revision whenever
provider/model version, prompt transformation, limits, pricing or other route
configuration changes; use immutable provider route identities for queued work.
Only opt in when reusing a nondeterministic model response is intended policy.
Set None to disable new bindings; already admitted calls retain pinned policy.
The cache key covers canonical whole request minus request/correlation IDs,
provider identity, logical model, coglet identity and revision. No cross-coglet
cache sharing. Successful outputs only; failures/uncertainty are not cached.
Hits bind the **new request ID**, settle at zero cost, release reservation and
produce an independent immutable reply/inbox record. For conservative consistent
admission, even cache hits must initially reserve the route ceiling. A depleted
budget can therefore refuse a request that would have hit cache. Concurrent misses
may both make a paid call; this is a response cache, not single-flight delivery.

Keep DB/WAL private: prompts, replies and cache entries are sensitive. SQLite
WAL/FULL transactions are durable; backups must include WAL. Existing skeleton
DBs migrate additively with separate policy/cache tables. Retention/eviction is
operator maintenance, not automatic deletion of recordings or uncertain claims.

## Verification

```sh
cargo test --locked -p coglet-llm -p coglet-mock
cargo clippy --locked -p coglet-llm --all-targets -- -D warnings
cargo fmt -p coglet-llm --check
```

Named tests cover budget exhaustion/ledger isolation (existing `llm_gateway`
suite), atomic rate admission/backwards clocks, isolated response-cache hits and
revision invalidation, cache-hit authorization, both provider formats over a real
local TCP fake, error mapping, ambiguous restart/no-resend, endpoint/pricing
refusal, and `recorded_call_replays_same_runtime_state_hash_without_provider`.
The last executes committed runtime request -> gateway recording -> exact inbox
boundary -> real runtime replay and compares each state hash; altered recordings
fail. The guest is the deterministic MockEngine; native-engine qualification and
full language-neutral hub/runtime conformance are **not** claimed by these tests.

Optional paid live smoke is ignored by default. Start
`~/.hermes/plugins/pool-llm/bin/hermes-pool-llm serve`; explicitly set its local
`ANTHROPIC_BASE_URL`, `COGLET_LLM_SMOKE_MODEL` and `COGLET_LLM_SMOKE_KEY` through
supported secret injection, then run
`cargo test -p coglet-llm --test providers optional_live_smoke_gated_on_secret -- --ignored`.
No direct-endpoint fallback and no real credential/call in the default suite.
