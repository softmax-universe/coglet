# ABI fixture coverage

Run `python3 -m unittest discover -s spec/tests -v` after installing
`spec/tests/requirements.txt` (Python 3.12 is the CI environment).
Regenerate data/schemas with `python3 spec/tests/generate_abi.py`.

| Section | Positive | Negative |
|---|---|---|
| Imports/exports contract | call-valid, action/timer/LLM/log timelines | call unknown fields, malformed timer time |
| Filesystem | snapshot-valid, log append | odd hex, unsafe-path-rehashed, code-write, missing log bytes |
| Inbox | inbox-message-valid, inbox-boundary | unknown fields, uncorrelated LLM reply (rehash control) |
| Budgets | budgets-valid, compute/wall recorded aborts, admission initial-memory-at-cap | zero-compute, oversized-batch, wall-error-code, admission initial/maximum-memory-over-cap |
| Determinism | golden CBOR, chained two-wake recording | corrupted tree/record/previous/initial/code/manifest hashes |
| Journal/trace | step-record-valid, trace-valid, llm-reply-trace | effect-on-abort, wrong effect list, partial frame tail |
| Manifest | manifest-valid | unknown fields, duplicate subscription, exclusive output not required |

`cases.json` contains dual-schema shape cases and valid deterministic-CBOR hex.
Temporal data uses JSON syntax with the `.fixture` extension, explicitly loaded
by test_abi in CI. These are not codec shape cases: a semantic-negative trace
can be schema-valid, and must not be mislabeled invalid to the recursive codegen
`*.json` corpus. No temporal fixtures are skipped by the ABI tests.
`semantic-cases.fixture` contains cross-field positive/negative records; tests also
exercise rehashed trace corruptions and framing. `trace-cases.fixture` contains
rehashed malformed inbox/reply/JSONL/wake cases, batching and trace-boundary wake
controls, plus historical completed effect-ID conflicts and fresh-ID controls for
all three effect kinds. Rehashed historical and same-step duplicates returning
invalid_argument, permission_denied or resource_exhausted must fail; authorized
conflict and fresh-ID controls must pass. Capability/identity denial precedes
dedup even for historical IDs. Fresh authorized calls may stage or exhaust
resources: these fixtures do not independently implement engine resource metering.
Both schemas validate the trace containers; semantic
validation must then reject each negative for its named reason, not a hash error.
The actual ABI schema roots are tested separately from selected helper rules.
`llm-reply-trace.fixture` is a
self-contained two-step recording with blobs, not invented provider measurements.
Its reply text is deliberately synthetic fixture data. Engine-specific budgets
and NaN/feature behavior remain TBD-by-ADR; no runtime has been certified here.

Path keys/blob digests and duplicate capability arrays are semantic requirements
in addition to shapes. The CDDL preserves common rules unchanged. pycddl 0.6.4
misapplies regex controls on arbitrary map keys, so both ABI shapes leave map keys
as text; semantic tests enforce path/digest constraints. It also mishandles some
heterogeneous nested common profile-map values: the LLM reply is checked against
its explicit reply rule and JSON Schema, and retained as exact inbox bytes in the
trace; the basic dual-schema inbox golden uses a steering payload. Do not weaken
the common profile or claim this tooling limitation forbids valid nested payloads.
The actual Wasm export signatures require module inspection by engine conformance;
data shape fixtures are not a module validator.
