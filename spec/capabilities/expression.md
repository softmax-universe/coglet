# Expression capability — 0.1

> **Names on this page.** The prose says *adapter* and *router* ([ADR-0020](../../docs/adr/0020-vocabulary.md)). Identifiers, file names and wire names still say `surface` and `hub` until the rename lands; they mean the same things.

Imports [conventions](../conventions.md) and [adapter protocol](../surface-protocol.md).
Decision: [ADR-0007](../../docs/adr/0007-surface-speech.md).
Schema rules are in `messages/expression.cddl` and `expression.schema.json`.
The normative [operation bindings](bindings.md) select set-expression args,
closed empty successful action result, closed empty state-read args and
expression-state read result; generic envelope/root-union validation is insufficient.

`expression.control` advertises action `expression.set_expression` and read
`expression.state`, with exclusive resource `display.face`. Action args:
`{expression, intensity, duration_ms}`. The action requires display.face
ownership as well as the operation grant. Expression is one of `neutral`, `listening`,
`thinking`, `speaking`, `happy`, `concerned`. Intensity is integer 0..1000 (never a
float). Duration is 0..60000 ms; zero resets to neutral/intensity 0 immediately
(the supplied intensity is ignored for this reset). A positive
duration expires to neutral on the native monotonic clock, even if the coglet is
hung. Deadline still controls action admission. Success means the renderer applied
the expression, not that the entire duration elapsed. A new accepted expression
replaces the prior state; timers are fenced so old expiry cannot erase a newer
expression. Read returns `{expression, intensity, remaining_ms}` at one snapshot.
Kill/revocation/disconnect resets neutral and prevents further writes.

This is a semantic profile, not a face mesh, LED address or product animation ID.
A Mac face maps to native animation, an Echo Dot maps to LED ring patterns, an
EO1 maps to display artwork; an explicitly configured no-visual renderer may
acknowledge semantic state without claiming a visible face. All support the same
vocabulary and state readback, so the identical Face Wasm and manifest run unchanged.
A body lacking a renderer must advertise no expression capability rather than
silently accept unsupported operations. Devices choosing the Face baseline provide
at least the semantic renderer. Persona/voice and hardware assets stay in products.

Missing grants fail permission_denied; unknown vocabulary fails invalid_argument,
not fallback to an arbitrary product command. No event is required: readback and
the common action outcome are sufficient. Golden valid/invalid payloads are in
`fixtures/expression/cases.json`; rendering appearance requires product tests and
is not proven by schema validation.
