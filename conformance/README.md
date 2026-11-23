# Conformance bootstrap

Run `./conformance/run` from any working directory. Python 3.11+ is required.
An empty `implementations.json` passes explicitly without claiming conformance.

The registry maps implementation names to nonempty argv arrays (no shell).
Commands run from the repository root, have a five-minute timeout, and a missing
command, invalid registry, or nonzero exit fails the check. Registered commands
must build/launch their implementation and run the shared black-box wire suite
against it; real and mock implementations must use the same suite.

The `abi/`, `surface/`, `hub/`, `engine/`, and `speech/` directories are reserved
for those suites. No wire protocol or harness is implemented in this scaffold.
The harness card owns the final registration interface. This bootstrap dispatch
format is tooling only, not a protocol decision.

Test the dispatch with `python3 -m unittest discover -s conformance/tests -v`.
