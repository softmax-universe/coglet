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
expires grant-bound barriers without depending on a transport poll. Missing or
failed reads fail closed by disconnecting the native session and purging its private
input; a read timeout cannot silently authorize reasoning on uncollected history.

Qualification limits: 128 simultaneous observations, 4096 read-ID tombstones per
connection and 4096 non-evicting archived items; no silent eviction or inferred text.
Provider-native items still fail closed until durable session/item ownership is
implemented. Full playback stream/position/barge pairing is the next gate; current
construction is not a claim of physical audio/provider alignment qualification.
Out-of-order snapshots that regress archived positions are refused, not rewritten.

`cargo test -p coglet-runtime --test native_operational native_heard --locked`
executes socket and actual C pointer reads plus revoked-late-reply checks and an
explicit-runtime-clock barrier expiry regression. Add `--features native-engine`
to execute real Wasm on those native routes: three position-consuming records each
include a prior heard boundary, and no read creates an extra event or step. Default
feature tests instead verify three zero-step/trailing boundary changes after reopen.
The reducer unit test covers exact multibyte prefixes, revision, regression, duplicate
stop and the complete stopped-item freeze.
