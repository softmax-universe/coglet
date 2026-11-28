# Existing-image transition — actual bounded hosted control

2026-11-28; **this increment PASS, not stage2 PASS**. Remaining final review,
required CI/native-queue main merge, and containing-main hosted acceptance are
not replaced by this evidence. Existing strict/private/factory paths remain separate.

## Source and tools

Actual successful hosted execution source `165a9a1ad1128c407f78ba5703955ffb3e0195a2`;
Unforged `c8aeece23b674824ed48f3ea39999a11a92f7173` (latest owner-directed pin).
Static native-only driver SHA256
`8113231a72ac1c0ecb94d022c34278ba7446950d757ed829469d2ac019ef9265`;
bridge `9caf04ca9d6c46e644cfc17f40dad9669fc799ef60979b8a685226942659925c`;
unforge `006b0cf4cc7ec5f6084e42742506bf9bf910df1b24c8736b1ee52da02e02f9de`.
Endpoint identity `e05401185f40e92e694c635fa75ba4a8cdef5bfcb0f7759f0713decfd6c3d6ce`.
Credentials only via broker `caos.prod.connect`; no keys/tickets in evidence.

A std/bash: `localhost:5000/caos@sha256:842b81ee7ab08908ab2cd09ad01c3727a5333c832c2fd76a2974339fc4bf31ad`.
B std/go: `localhost:5000/caos@sha256:de985d089552ab9de870c54e6e3e9d028300b517bf7ee389fe1079808b4a4421`.
Existing locator provenance and earlier actual capacity probes are in
NO_EFFECTS_EVIDENCE.md and its archive. No image pushes, deployments, workers,
new capacity, GPU or paid service were needed.

## One client, unchanged consumed resolver/program/input

One actual `CaosClient` submits A-cold → B-first → B-warm → A-return. Only
`CaosBaseSpec::Docker` changes. The same polyglot worker invokes the same launcher,
which executes the actual `ResolverConfig`-selected executable from the pinned
resolver. It runs genuine recorded model step1, full prefix/recorded reply closure,
not a fabricated filesystem script. Same salt, state, driver, args and bundle.

Resolver commit `e0a5b1715f84514f16713753222e138997fd3df3dad7519ce95428608e30709c`;
args `c0d400a45bd1ca88f8848612620c1980317e9583`;
bundle `5f21b43e18a916a82f5d986d10627f2aeb7b8a84`.
A request `4435634288a5eaab5b97343f1b5e759f6ac7764f`;
B request `e0826d2f08204994a5be4b6f971cdba2ccd55ca3`.

| invocation | actual X-Caos-Cache | actual resolved X-Caos-Image |
|---|---|---|
| A-cold | miss | A exact immutable digest |
| B-first, ordinary request without nocache | miss | B exact immutable digest |
| B-warm | hit | B exact immutable digest |
| A-return | hit | A exact immutable digest |

All four complete output graphs recursively derive ABI SHA256 tree
`4df427705abb9aec15df95bea0cc782fa8d59b5d958ab2355a400e7d64fabbad`, equal the genuine native oracle.
Result **2 executions / 2 remote hits / 2 distinct request identities**. No hidden
B warm-up or fresh-client substitution. These are result-cache observations, not
OCI pull cache, physical hardware identity or local memo invalidation claims.

## Exact reproducible commands and independent checks

`image_probe.py --synthetic-transport-only --driver <static-driver> --bridge <bridge>
--unforge <unforge> --git <private-Git>=2.45 --output <fresh-exclusive-directory>
--driver-sha256 <above> --bridge-sha256 <above> --unforge-sha256 <above>
--endpoint <above> --image-a <above> --image-b <above>`.
Run through broker env injection; retained `command.json` records every exact path,
value and argument. No endpoint ticket is an argument. `qualification.py` now
invokes this fresh probe then the independent verifier in the named
`hosted_changed_image_invalidates_acceptance` entrypoint; no historical evidence
can substitute for execution. Optional closed `image_controls` config carries
`git`, `unforge`, `unforge_sha256`, `upstream`, `image_a`, `image_b`; absent config
fails closed. Existing six entrypoints retain their strict/native/grant mechanisms.

Offline: `python tests/unforged_proof/verify_image_probe.py --batch <archive-root>/hosted-image-final
--source 165a9a1ad1128c407f78ba5703955ffb3e0195a2 --upstream c8aeece23b674824ed48f3ea39999a11a92f7173
--driver <above> --bridge <above> --endpoint <above> --image-a <above> --image-b <above>`.
Needs the committed execution source reachable in this repository to independently
bind actual worker/launcher bytes and modes, not just self-reported hashes.
Checks every request raw object's SHA1, complete closure, sole base delta,
resolver commit+complete tree+selected ELF, exact prior state and full recorded
payload, submit body/response, actual per-invocation run header pairs, completed
status/request/sidecar/salt bindings and every complete output tree.

Independent reviewer sa-0-889fff01 reran actual evidence PASS and source bytes
match, but found a verifier hole: coherent worker/launcher replacement still
passed. Three new regressions genuinely failed RED missing binding function;
fixed source-bound exact bytes/modes. Parent reran the actual reviewer controls
normal and -O: healthy passes; raw request corruption, raw output corruption,
wrong status binding, wrong nocache invocation, coherent worker replacement and
coherent launcher replacement all refuse. These are LOCAL consistency mutations,
not claims of hosted tampering or new worker deployments. Original sequence tests
were test-first; the first offline verifier implementation was not test-first.

137 fast Python tests normal/-O PASS; latest Rust bridge 7 PASS/1 ignored explicit
cross-language consumer (not silently counted). The original 124 fast tests and
baseline source inventory remain. Final review still required.

## Honest failures and limits

`hosted-image-reviewed` first A-cold failed: CAS-derived copied files were read-only.
`00b49e1` makes only private owned copies writable before executing. Next
`hosted-image-corrected` failed because generic Docker workers do not expose native
`CAOS_OWN_ARGTREE_HASH`. `165a9a1` binds sidecar via actual completed status request
and its fresh salt instead. Both failed transcripts retained, not counted PASS.
Exact successful bridge was independently reviewed before acquisition; verifier
follow-up review does not relabel earlier source commits.

Generic Docker bounded-native replay is explicitly synthetic/public; this does
not establish strict sandbox isolation or authorize private transport. Those are
qualified by separate existing strict/grant paths. The audited native driver
records model/adapter outcomes; its no-live-dispatch closure remains independently
pinned in NO_EFFECTS_EVIDENCE. No universal syscall accounting claim.

Durable `evidence/hosted-image-interim.tar.xz`: 21,011,488 bytes, 19 members,
SHA256 `c3c086db356858622295eadf07dd0330204e4b87167dce204ff4804186f6143e`.
Contains raw successful/failure transcripts, original/regenerated native traces,
resolver commit, command/config/pins, observations and offline/mutation results.
Every archived member was read back and byte-equal to the original. Reproducible
OS stores, source snapshots and private authority stores are omitted. Below25MB.
