# Live heard collection (bounded native route)

Both service-backed native transports use the same runtime policy: an authenticated
`speech.position` or `speech.barge_in` observation, when `speech.heard` is advertised,
initiates a runtime-owned read of that item. There is no new surface message or guest
import. The generic library `request_read`/`take_read_result` seam remains available
to trusted supervisors; automatic heard replies are consumed by the runtime, not a
guest result queue. Manifests requiring history must require `speech.heard`; native
scheduling already refuses absent required operations/resources.

Event source highwater, private-event admission and a bounded read barrier commit
in the same WAL transaction. All `run_next` calls wait while a barrier exists, so
neither a position-consuming step nor an unrelated reasoning wake can consume
history before collection. Requests have a one-second runtime deadline and current
grant epoch. Results recheck durable epoch, current operation and speaker lease,
retained accepted item ownership and selected schema/UTF-8 caps. A live read may
advance beyond its triggering position; a stopped/final read must equal the frozen
cutoff. The read itself is the alignment evidence, never proposed speak text.

The M-D reducer records maximum measured duration, monotone played position and
append-only UTF-8 prefix suffix spans. Empty, shrinking, unchanged or revised text
adds no span; stop freezes the whole item. Each changed snapshot creates a stable
`runtime.heard` mutation at `/data/runtime/heard.cbor`. Removing the barrier and
staging that mutation share one fsynced commit. It queues no wake or synthetic
event. The next real step incorporates the boundary; an outbox-only host can seal
it as a trailing/zero-step boundary. Replay uses archive bytes only.

Revocation, disconnect and restart purge abandoned barriers and their private
queues without rewriting previously journaled snapshots. Runtime scheduling also
expires grant-bound barriers without depending on a transport poll. An explicit
negative heard reply still disconnects and purges private input. A local read
timeout instead completes negatively through the trusted native `take_read_result`
API without an early disconnect (ADR-0029); its durable heard barrier stays closed
until normal authority/disconnect cleanup. A late reply is discarded, never used
to invent history or authorize reasoning. See
[read-expiry qualification](READ-EXPIRY-QUALIFICATION.md) for result pressure,
session-local retention and diagnostic boundaries.

Qualification limits: 128 simultaneous observations, 4096 read-ID tombstones per
connection and 4096 non-evicting archived items; no silent eviction or inferred text.
Playback observations retain item/stream owner, epoch and chunk bindings, stream
and item position highwaters, stop fences and exact barge/position pairs. Identical
barge retransmission after later capture/playback does not roll back or wake again.
Stopped streams cannot admit a newly observed item; ducked items can progress.
These observations, source highwater and private enqueue commit atomically.

Provider-native items bind to the sole historically prepared session-open on this
surface, which must have a successful outcome in the current epoch and no accepted
close. Multiple opens (even a close/reopen or retry under a NEW ID) make previously
unseen native items ambiguous: the frozen position wire has no session field, so
the runtime refuses them rather than attributing a delayed old callback to a new
session. This conservative bounded route requires supervisor lifecycle/identity
reconciliation for further provider sessions; it is not unrestricted provider
reconnect support. Existing items retain their original owner/epoch/session.
Tool results require a retained call and live session; a previously dispatched
result fences new IDs while admission is unknown or accepted. Explicit rejection
allows a new ID; identical old IDs still reconcile their original envelope.

Concurrent heard replies may complete in reverse order. Authenticated older read
snapshot revisions or positions release only their barrier, without rewriting
history; replies after a stopped archive never fill or extend it. All barriers
must finish before reasoning. This is not physical audio/provider qualification.

`cargo test -p coglet-runtime --test native_operational native_heard --locked`
executes socket and actual C pointer reads plus revoked-late-reply checks and an
explicit-runtime-clock barrier expiry regression. Add `--features native-engine`
to execute real Wasm on those native routes: three position-consuming records each
include a prior heard boundary, and no read creates an extra event or step. Default
feature tests instead verify three zero-step/trailing boundary changes after reopen.
The reducer unit test covers exact multibyte prefixes, revision, regression, duplicate
stop and the complete stopped-item freeze.
