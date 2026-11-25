# Local initial-deploy saga

This coordinator implements ADR-0018's local operator initial-deploy path. It
uses the existing operator socket/UID provisioning, qualified finite builder,
router Store, authenticated iroh Session and runtime durable receipt boundary.
It adds no wire methods or identifiers. Installed-target activation/hot update
is **not** qualified here.

## Startup and ownership

The protected router startup JSON accepts `recipes: [Recipe, ...]`. Each recipe
contains `target` (builder Target), `profile` (qualified Profile), `delegation`
(Delegation), `generation`, `validator` (executable path), and `quarantine`
(directory). These are existing builder types; see `src/build.rs` and the
qualified builder tests. Recipes are not authority supplied by the socket
request. The configured router identity signs; no coglet key enters the builder.
UID provisioning must separately allow the local operation and current grants
for its exact runtime, target and methods. Missing enrollment is
`unauthenticated`, missing method/target/expiry authority is `permission_denied`,
and a different installed commit is `conflict`. These are existing error codes.

`Service::with_executor` enables execution; production startup owns and joins its
background runner. Socket client deadlines only stop waiting. The durable
admitted work continues without a socket waiter. A committed enrollment makes a
Session eligible for dispatch; no unsolicited health/event request is required
from an idle runtime. Independently ordered QUIC streams can deliver a reverse
request before the runtime consumes enrollment selection. A peer's
`unauthenticated` refusal therefore leaves the operation queued, with current
local authority rechecked on the next pass and original-ID receipt reconciliation
before any mutation resend. Local missing enrollment still returns its typed
terminal error. Disconnect removes only the matching connection generation.

## Durable custody and recovery

Admission records both the canonical timeout-independent local intent and its
provisioned actor. A changed actor mapping cannot adopt historical work. The
builder retains exact signed metadata and bundle before any dispatch; the saga
retains its complete child envelopes before I/O. Deterministic original child
IDs are scoped by UID/local operation ID/stage. Runtime effects are not inferred
from local queued state, accepted refs, or status.

The stages are status preflight, streaming `hub.git.push`, `hub.install`,
`hub.start`, and a final running/commit status observation. Only an actual applied runtime receipt permits the next mutation.
After an attempted mutation, recovery first reads `hub.receipt` with the
ORIGINAL child ID, actor, target and absent-ticket omission. Fresh receipt and
original-method authority is checked on every transport poll. Only an actual
`not_found` allows an identical original-ID resend; ambiguous/queued outcomes
remain queued. No new mutation IDs or synthetic successful runtime receipts are
created. Terminal failures/cancellation and all prior child observations remain
in the existing Store/audit. Current-authority loss fences further dispatch;
it does not pretend to undo an effect that may already have happened.

`status` retains its original observation on local-ID retry; it is not a runtime
operation receipt. `stop` uses the same durable mutation/reconciliation path.
Different intent under the same local ID conflicts. An installed different
commit is rejected before a new deploy push rather than being replaced.

## Requirement-to-test matrix

Run on Linux (the fixture uses actual Unix peer credentials):

```
cargo test --locked -p coglet-hub --test operator_saga
```

| Requirement | Named test / assertion |
| --- | --- |
| Actual socket → qualified build → streaming Git → install → start → running | `operator_real_iroh_deploy_running_stop_status` asserts actual runtime status is running |
| Lost coordinator at each deploy boundary | `operator_saga_crash_each_stage`, five barriers: post-build/pre-push (`hub.status`), post-push, post-install, post-start, and final running observation |
| Original child IDs and exact scope after recovery | crash test compares all retained envelopes, observes real receipt polling and one distinct ID per mutation |
| Immutable local retries and conflicting ID reuse | running/stop/status test compares terminal responses and rejects changed source under the same ID |
| Original-actor receipt privacy | same test has separately granted wrong-actor denial plus authorized original-actor applied control |
| Stop/status and retained observation | same test stops the real runtime and retains prior status on identical retry |
| Typed missing enrollment / missing grant / conflicting install | same test revokes enrollment/grant and tries a distinct pinned publication against an installed target |
| Admission while executor is blocked | crash test admits another actual socket request while the runtime reply is paused |
| Build does not hold global State | running test pauses the real validator and obtains a concurrent kernel status snapshot |
| Retryable peer pressure | running test injects one pre-admission busy refusal, then requires real original-ID receipt recovery and actual install success |

`coglet_mock::lifecycle::LifecyclePeer` supplies actual authenticated iroh
connections and accepts an OperationHandler; it does not generate receipt
bodies. These tests supply the production runtime DeviceHandler with its native
WAMR engine, actual durable storage, real Git quarantine validator and signed
pins. The test wrapper pauses **after** the real handler result and discards the
coordinator before it can retain that reply. Recovery constructs a fresh
Kernel/SQLite connection and socket service against retained disk state.

These are coordinator task-abort/reopen crash simulations, not SIGKILL of a
router OS process. `operator_cogletd_deploy_running_stop_status` additionally
runs the **same** initial-deploy/retry/stop/status and typed-error exercise against
an actual native `cogletd --router-service` subprocess and the production router
connection handler, without manual session attachment. It kills and reaps that
runtime after stop, restarts it with null-invite enrollment recovery, and observes
its durable stopped state and unchanged local receipts. It does not qualify
resuming a running guest after runtime restart. Execute explicitly (this binary
acceptance and the privileged UID test are ignored in ordinary Cargo runs):

```sh
cargo build --locked -p coglet-runtime --features native-engine --bin cogletd
COGLET_TEST_COGLETD="$PWD/target/debug/cogletd" cargo test --locked -p coglet-hub --test operator_saga operator_cogletd_deploy_running_stop_status -- --ignored --exact
cargo test --locked -p coglet-hub --test operator_peer_process operator_peer_uid_nonowner_subprocess -- --ignored --exact
```

The real binary test first failed with a permanently queued deploy before the
enrollment/session attachment fix. The mock exercise separately injects a
non-retryable pre-handler `unauthenticated` refusal, then requires actual runtime
success (no injected successful receipts). Its conflicting-install control still
requires a terminal failure; local grant/enrollment controls still require typed
errors.

## ADR-0018 security matrix

The ADR's requirement names map to these executable cases, rather than aliases
that conceal which assertion runs:

| ADR requirement | Executable case |
| --- | --- |
| `operator_peer_uid` | `operator_peer_uid_nonowner_subprocess`: actual root peer rejected before audit, service-UID positive control |
| `operator_socket_permissions` | `operator_socket_permissions`: private socket and real OS peer credentials |
| `operator_fail_closed_creds` | `operator::socket::tests::operator_fail_closed_creds`: unavailable credentials and foreign/root UID decisions; actual acquisition/rejection exercised above |
| `operator_frame_limits` | `operator_frame_limits`, `operator_socket_frame_bounds`, and bounded server tests |
| `operator_idempotency_conflict` | `operator_idempotency_conflict` and both lifecycle backend exercises |
| `operator_status_snapshot_retry` | `operator_status_snapshot_retry` and lifecycle status-after-stop immutable retry |
| `operator_saga_crash_each_stage` | same-named test: five coordinator abort/reopen barriers, original child IDs and actual receipt polls |
| `operator_receipt_audit_atomicity` | same-named test plus `operator_grant_receipt_audit_and_authority_share_one_commit` and production socket rollback controls |
| `operator_revocation_before_dispatch` | `operator_socket_revocation_resets_blocked_real_iroh_write` plus lifecycle grant-revoke denial |
| `operator_invite_hash_only` | `operator_invite_hash_only_restart_and_window_retry_stay_closed`, known-token TTL/close controls and production socket audit rollback |
| `operator_no_remote_listener` | same-named real-iroh test: all eight local operations sent raw after enrollment reset with code 1, no audit change, local socket positive control |

No physical device, iOS execution, hot update, or live deployment is claimed. The
unchanged router event conformance runner qualifies only its ten named event
cases, not the full protocol or all runtime behavior.
