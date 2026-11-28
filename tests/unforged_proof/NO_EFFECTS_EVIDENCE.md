# Narrow replay closure and existing-image availability — NOT stage2 PASS

## Implementation/review

At Coglet `919c51a784c4d10d9deb82a9400dc37f85950867`, moved the existing CLI
`prepare` branch unchanged into supervisor-only `coglet-unforged-prepare` crate;
updated stage1 harness/build instructions. Both driver and visualizer runtime
edges disable default `device` features, retain `native-engine`. No runtime,
journal, adapter, scheduler or Unforged primitive changed. Independent plan
APPROVE sa-0-e2a9a98b; exact919c51a source/increment APPROVE sa-0-4b79f0d4.

The graph guard first refused the real pre-cut graph (CLI/proto/iroh/HTTP/socket
packages), then passed the native-only normal selected graph. Four LOCAL parser
cases first RED missing module, then GREEN. Three source-byte-binding cases first
RED missing module, then GREEN. Additional qualification ordering test was added
after implementation, not test-first. Original116fast retained;124normal/-O PASS.
Programmatic baseline95files/453definitions retained, including after upstream bump.
No unrelated dependency version upgrades: remaining registry package versions and
checksums unchanged in both trimmed lockfiles. Prepare has its own larger lock;
that helper is never uploaded as the hosted driver.

Source audit: actual bare driver -> Runtime.import_trace -> Runtime.step ->
NativeEngine -> StepHost has no reachable live provider/adapter dispatcher.
StepHost records model requests/actions; recorded outcomes enter as boundary data.
WAMR uses virtual Snapshot WASI handlers, no socket imports, built-in libc/WASI
and pthread disabled. Other compiled runtime service/WAL/local socket code is not
entered by that step path. Core Tokio remains; intentional std::net loopback
canary subcommands remain. No claim of universal no-syscall/zero-provider counters.
Build-time WAMR fetch is separate and checksum verified by existing build.rs.

Reviewer found unpinned WAMR wasi.def; fixed immediately in
`47cc741cef047ad2dc1a8488dffedc4cc68ef975`, also binding root Cargo.toml,
.cargo/config.toml and rust-toolchain.toml. Exactly236source files pinned.
Independent exact47cc741 APPROVE sa-0-7a65b4a9 independently verified all pins,
actual graph and124tests normal/-O. Pins are committed/review-bound, not an
operator boolean or self-authenticating approval. Tracked-file selection is not a
blanket untracked/build-input security policy. Changed source requires renewed
review; changed binary still requires actual hosted proof.

## Actual changed-driver proof (old upstream pin, never relabeled)

`hosted-no-effects-cut` ran ONLY `hosted_replay_has_no_live_effects` at47cc741,
Unforged `f9c627bcc09346b2923583037a10ac9a2fa8ea12`, before David's later bump
instruction. EXIT0;2genuine model+9Gridbot replay =11unique actual hosted jobs.
Every fresh salt/cache miss/exact native image/raw status/sidecar/full returned
filesystem graph matches its native recorded oracle, with mandatory real loopback
controls. Offline helper reread all11 raw transcripts/graphs and rehashed exact
whole ABI trees. Model has a genuine recorded llm request/reply; Gridbot has
nonzero actions. Complete original traces retained, no telemetry normalization.
This was new changed-binary execution, not rerunning a completed old batch.

Actual pins:
- static driver `8113231a72ac1c0ecb94d022c34278ba7446950d757ed829469d2ac019ef9265`
- old bridge `67fad1c04891eec9414a0d76b66b57fed7d6ab31d606ee2603d5fd904c1324dc`
- native image `native@sha256:2e92d104eed5ad02c0daa765c4b17aa35f6be942e39248ef53fdd4987b429ff5`
- endpoint `e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce`
- requests and complete command/tool pins in offline-verification.json/config.json.

Both visualizer lib and bin compiled successfully without device (zero unit tests,
not coverage claim). Changed driver passed6explicit native replay and3canary tests.
After latest-Unforged rebuild, the actual original stage1 harness also EXIT0:
5cold fulltree matches and5warm memo hits, no unsafe-unsandboxed. This regression
was necessary for extracted preparation, not a hosted batch rerun for upstream bump.
Old driver's prepare and new supervisor helper on the SAME actual CLI-built package
both exit0 and produce EXACTLY equal job bytes (prepare-equivalence.json).
Task-private Git2.48.1 with reftable was rebuilt; system Git unchanged.

## Scope-owner-directed existing images

Following David's08:31 clarification, actual authenticated `/eval` and
`/resolve-image` resolved two already-published immutable std images at CAOS
locator commit `7f0efc5b73ce61bf66dff37bb209901a83bc8dec`:
- std/bash: `localhost:5000/caos@sha256:842b81ee7ab08908ab2cd09ad01c3727a5333c832c2fd76a2974339fc4bf31ad`
- std/go: `localhost:5000/caos@sha256:de985d089552ab9de870c54e6e3e9d028300b517bf7ee389fe1079808b4a4421`

Independent sa-0-6a8a1181 capability probe, then sa-1-bcd3e18d actual bounded
synthetic jobs on both images, one persistent authenticated Iroh connection.
Both HTTP200/X-Caos-Cache:miss; X-Caos-Image equals exact submitted digest. Parent
actually reran verify.py over raw receipts+output objects, exact SHA1 and fresh
nonce bytes PASS2. Distinct bash/go worker1 source means this is NOT same-program
A/B qualification, not genuine Coglet step proof. Base-only native503 does not
exclude pool-scoped native advertisements. Initial client formatting errors remain
honest errors, not service refusals. No deployments/newworkers/registrypushes.

Remaining A/B must keep ONE actual CaosClient and SAME resolver/program/input,
change only base A/B, require B fresh execution+digest, B warm hit, A return own
entry. Two newly constructed clients do NOT satisfy same-client requirement.
Concrete source review sa-1-f51301da: highlevel hardcodes bash worker1 (std/go
cannot interpret it), runner client private/immutable. Direct existing STANDARD
API can keep one client and same polyglot bash/Go worker1 invoking same static
binary, but complete actual-step/privacy/full-tree and supported resolver binding
must be implemented/tested, not invented. Do not add a second cache/scheduler,
rebuild CogletTree by projection or claim synthetic nonce jobs meet acceptance.
Capacity wall is superseded by actual successful existing generic image dispatch.

## Latest Unforged instruction, before any subsequent batch

David08:52 requested latest upstream. Fetched/fast-forwarded clean checkout to
`c8aeece23b674824ed48f3ea39999a11a92f7173`. Existing bridge compiled locked/offline
without API edits; rebuilt unforge and static driver; retained124fast normal/-O
PASS again. Static driver has no Unforged dependency and rebuild gives same8113
bytes. Never pass --unsafe-unsandboxed. No completed old batch rerun for bump alone.

New local build pins (not hosted qualification):
- unforge `006b0cf4cc7ec5f6084e42742506bf9bf910df1b24c8736b1ee52da02e02f9de`
- bridge `1fe4b37762d2a0ed6fb2aa49218207e6cbc552643043b237cdefb71ae0d01c83`
- static driver `8113231a72ac1c0ecb94d022c34278ba7446950d757ed829469d2ac019ef9265`

Old11step evidence above stays oldpin. Final acceptance on newpin, final wholebranch
independent exactheadreview/requiredCI/nativequeue/main/containingmain and final
evidence handoff remain. No stage2PASS or stage3 release.

Durable interim archive: `evidence/hosted-no-effects-interim.tar.xz`,454016bytes,
245members,SHA256`b7c4405dc098b25bd7306a208b3425bdefa224356b91eac8f2cf1997737d8b91`.
Entire tar member readback matched original raw bytes. Includes11rawjobs/oracles,
source/config/commands/graph, two image discovery+execution probes, native stage1
trace/results/rawreceipts, helper equivalence and baseline inventory. Omits only
reproducible per-step snapshot/prefix copies and object stores; no credential keys,
tickets or authority stores. Archive document predates this receipt paragraph.
Final native artifact attachment remains part of final review handoff.
