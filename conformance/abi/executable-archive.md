# Executable archive companions (ADR-0015)

Added only after ADR PR [#76](https://github.com/softmax-universe/coglet/pull/76)
merged as `f625b569fa71f61ed1a02345f5cd95eef34949d8`.

`executable-archive.json` adds 42 harness cases. `executable-archive.fixture`
is the exhaustive source-case/record inventory, including pinned WAT sources,
Wasm bytes, runtime-only boundary positions and negative obligations. Neither
file replaces any of the 128 frozen ABI cases. All 80 source archives and all
32 guest records (21 boundary, 8 provenance, 3 heard) remain accounted for.

## Build and qualification

From the repository root (Cargo.lock pins the `wat` compiler):

```
cargo run --locked -p coglet-runtime --features native-engine --example generate_archive_companions
cargo test --locked -p coglet-runtime --features native-engine --test executable_archive -- --test-threads=1
python3 crates/coglet-runtime/tools/check-abi.py
```

The builder's `program` function is an independently specified scenario program,
not a compiler/interpreter for recorded calls or outputs. It emits WAT with an
explicit motor intent and step-dependent commit/return behavior. The pinned WAT
can independently be compiled with `wat::parse_str`; the inventory test compares
those bytes against both the source recipe and each archive's code digest.

Generation runs real WAMR imports to obtain new journal records, provenance,
manifest/code pins and core-deterministic CBOR tree/chain hashes. Wall duration
is an actual nondeterministic observation, so regenerating archives can change
record hashes. Module builds are reproducible; regenerating journals is not a
promise of byte-identical wall observations. Replay compares deterministic
usage, effects, results, status and state, while independently enforcing wall
budgets. It preserves original record identities for multi-step chain links.

Qualification runs the five ADR-named tests and requires their exact inventory
and zero failures/ignored/filtered tests. Missing native WAMR or missing results
fails the checker; default-feature workspace tests alone are not qualification.
The harness's archive operation remains deliberately nonexecuting. The checker
separately runs native replay and writes `target/runtime-all-abi/native-replay.xml`.
It verifies the fetched spec-v0 tag as well as pinned frozen file checksums.
CI's unfiltered conformance job fetches tags with `fetch-depth: 0`, runs this
checker, and uploads both archive and native JUnit reports. No network is needed
during qualification once build dependencies and the tag are present.

## Invalid archives are not executed

The 52 original negatives are still admitted through archive validation and
rejected before any native guest entry. Every guest record in an invalid source
also has a valid executable baseline mapping; all 32 mapped guest steps really
run. The inventory identifies the corruption and original source case. For
invalid trailing boundaries the baseline keeps the guest inputs and omits the
invalid trailing receipt. For malformed provenance the baseline pins valid
provenance. For forbidden guest heard writes/creates the valid guest is a no-op;
only legitimate runtime.heard boundary changes survive. An aborted emit cannot
produce a terminal receipt, so its baseline omits that impossible receipt.

Original negatives preserve their semantic assertions; each new companion also
has a final-hash corruption rejection control. The program-mismatch test pins
an aborting guest and recomputes provenance, snapshots and record hashes around
a purported committed no-op journal: archive consistency accepts it, but fresh
WAMR execution rejects it as an actual replay mismatch, not a digest failure.

No authentication, live device, provider, WASI filesystem or dispatch claim is
made. Runtime-only pre-step/trailing/zero-step boundary records are validated,
not counted as guest invocations.
