# r3-scalar-nan-v1

Protected local publication profile. Default remains `integer-WAT-v1`.

This profile is narrower than a general R5 claim. It admits core Wasm with the
same validator feature set as production engine admission (floats, mutable
globals, sign extension, saturating float-to-int, multi-value, bulk memory,
reference types). It rejects SIMD, relaxed SIMD, threads, shared memory,
memory64, atomics, GC, components, start sections, and any import outside the
frozen R4 allowlist. The currently supported WASI subset is args/environ get
and size queries; fd_close/datasync/sync/fdstat/filestat/prestat/prestat_dir_name;
fd_read/write/seek; path_open/create_directory/remove_directory/unlink_file;
and proc_exit, with the exact frozen signatures. Other valid R4 calls are refused.
Limits are applied to both original and expanded output: default 1 MiB module,
100,000 operators and 4,096 declared locals per function. Thus expansion near a
limit may refuse a previously small input, never silently exceed the output cap.
The digest record retains the actual configured limits. Custom/name sections are
retained; original local indices do not move when new locals are appended.

The rewriter is a pure function: original bytes plus these profile bytes and the
rewriter source digest yield output bytes and a digest record, or a refusal. The
record is not a credential. Unsigned, stale, foreign or identity-only evidence
never unlocks publication.

R3 wrapping uses the exact core-Wasm sequence required by the engine validator:
after each result-producing scalar float operator, `local.set t; local.get t;
nan; local.get t; local.get t; f.eq; select` with temps that do not alias
original parameters or locals. Bit-exact operators are not wrapped. A module
with no affected operators is returned byte-identical to the input.

Unexecuted architectures, engines, AOT and toolchains stay refused.
