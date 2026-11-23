# Database engine-seam case contracts

These are executable harness timelines for the proposed ABI 0.2 extension in
[database.md](../../spec/capabilities/database.md), not a new production transport.
Use the existing harness `--suite database --peer-role guest` and u32be CBOR
adapter framing. Each case starts a fresh private DB at revision 7, schema 1,
with approved catalog pinned; no network worker runs. The `fixture` member of
`db.step` selects this adapter-only setup, not a guest-supplied database name.

`private-goals-v1` contains ordered object IDs a,b,c of type goal. `objects_page`
selects them ordered by unique ID with one text type parameter. The literal
`opaque-owner-revision7-key-b` is test setup's authenticated cursor after b;
production cursors must be authenticated and opaque, not this string. The
`force_conflict` statement is a deliberately conflicting optimistic update.
Grants are the adapter's authenticated admission setup; real implementations
must provision the same identity isolation, never trust this array from a guest.

`db.step` supplies calls and termination at the engine seam; the adapter executes
these through the same DB import used by a guest. It reports observed replies,
step status, before/after revisions, and the list of dispatch-eligible intent IDs
after journal publication. No expected reply is included in the request. Reported
dispatch IDs mean eligibility, NOT physical external execution. Future real and
mock implementations use the same cases; no implementation is registered by this
spec PR. The suite does not qualify a Wasm loader, SQLite durability or real
worker execution.

Cases cover bounded rows, a second cursor page, whole-unit rollback after a failed
statement, one intent under identical duplicate keys, and conflicting reuse abort.
`spec/tests/test_database.py` feeds explicit scripted response frames to the actual
harness `timeline`, including missing/wrong responses and weakened assertion
negatives. Those passing tests establish **case contracts only**, not a DB host.
Phase 1 additionally owns real grant isolation, paging cursor authentication and
expiry, revision conflict, row/byte/time/call budgets, FK/version constraints,
crash prepare/marker/publication fencing, and backup restore qualification.
Phase 2 owns real dispatcher uncertainty, receipt and inbox restart qualification.
