# Private native DB substrate (in progress)

This crate is the first implementation slice of ADR-0013, not a claim of ABI 0.2
support. Existing runtime and engine admission remain unchanged and reject DB
guests. It must not be enabled or shipped as the Factory implementation yet.

Implemented and exercised against bundled SQLite:

- Private host-root database per authenticated owner, hashed directory name,
  restrictive Unix permissions, symlink checks, exclusive owner lease, foreign
  keys, WAL and synchronous FULL configuration.
- Immutable typed catalog statement IDs, exact positional parameter/result types,
  result names, per-statement read tables and a separate mutation target; canonical
  CBOR/SHA256 digest binds all metadata and SQL. The public approval route requires
  typed metadata. Conservative single-table reads prove a nonnullable unique key
  against the actual SQLite schema, including effective BINARY ordering; partial,
  expression and nullable unique indexes do not prove a tie-break key. Separate
  read/write grants and SQLite authorizer isolation protect schema/host metadata.
- Dedicated enqueue with pinned host intent grants, canonical-CBOR payload storage,
  same-key/content lookup before proposed-ID uniqueness, retained original IDs and
  conflict poisoning. Actual Factory schema/catalog integration is tested, including
  FK dependency reads without widening mutation authority. This is provisional
  storage only: no production commit, dispatcher, publication fence or receipt path.
- Short RAII rollback units, sticky failure poisoning, affected-row comparison for
  optimistic updates, a 32-call limit and cancellation inside SQLite at a supplied
  absolute host deadline. No public successful commit or schema provisioning API
  is exposed before the prepared-decision recovery protocol is implemented.
- Row/column/cell bounds and canonical-CBOR whole-page bounds, including empty
  results. The largest revision encoding is conservatively reserved. Overflow
  fails closed; this is not the final authenticated pagination implementation.

`cargo test -p coglet-db` executes the native substrate tests. Their acceptance-like
names do not establish shared black-box runtime/engine conformance. Tests use
`cfg(test)` provisioning and commit helpers, absent from production builds.

Required before task acceptance:

1. Integrate the typed catalog digest with code/schema/manifest admission. Catalog
   construction alone is not operator approval or ABI support. The conservative
   SQL subset is intentionally not a general portable-SQL parser.
2. Authenticated revision-bound cursors and admitted-attempt expiry; exact pinned
   revision accounting; ABI input/output and aggregate host-byte reservation.
3. Prepared step/DB marker/exact journal publication, restart recovery at every
   crash point, and offline recorded-result replay without a DB connection.
4. Integrate immutable-key enqueue with revisions and journal-publication dispatch
   fencing. Current native tests use cfg(test) commits; they are not engine
   acceptance or crash qualification. External dispatch stays outside phase 1.
5. Factory Wasm schema/repository and guest SDK import; explicit approved forward
   migration, verified backup/restore and schema compatibility checks.
6. Runtime/engine opt-in admission and real shared conformance adapters, all named
   acceptance cases in required CI, independent exact-head review and merge.

Hardening to retain during integration: host root must be private and trusted;
path checks are not a sandbox against concurrent mutation by a principal already
able to write that root. Database size quotas and SQLite connection limits need
explicit deployment policy. Configuration PRAGMAs must be read back before the
final durability qualification, rather than inferred solely from execution success.
