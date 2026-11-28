# Views and focus foundation (adapter slice 1/3)

The installed product and the DEBUG native wire qualification select only:

- reads: `softmax.app.state`, `softmax.app.controls`;
- events: `softmax.app.view_changed`, `softmax.app.focus_changed`, `softmax.app.window_changed`;
- actions: `softmax.app.navigate`, `softmax.app.open_window`, `softmax.app.close_window`, `softmax.app.set_focus`.

The reviewed 13-operation catalog, distribution bytes, and pin remain unchanged.
Explicit installed requests for other operations fail rather than silently widening
the slice. Native registrations remain available for complete tree observation;
a registered callback does not itself authorize its mutation operation.

## Deferred acceptance and source custody

Control invocation end-to-end proof belongs to adapter slice 2/3. Settings writes,
sensitive-capability gating, and credential secrecy qualification belong to slice
3/3. The former broad native UI proof and peer are preserved at commit
`8289f3d147f4b7a7df10b7f22450acc727994996` on
`foundation/softmax-app-views-focus-t_01a2212a69aa60b2` (and its parent
`7eb530492c914fe2e3b8e821149f04289ea62f44` on `test/softmax-app-live-wire-t01`).
The native UI test in this slice deliberately does not invoke controls, write
settings, seed credentials, or qualify sensitive dispatch. A dormant DEBUG
credential fixture is retained for source custody and Release exclusion auditing;
it is not selected by the foundation proof and is not an acceptance claim.

## Evidence boundaries

`AppSurfaceWireTests.test_softmax_app_observe_views_and_focus` drives the real
Swift SDK socket against the coglet-mock runtime peer and independently checks
native Settings navigation/focus. Event assertions use post-action frame marks,
not arbitrary historical matching. The test also checks tree reads, exact retry
receipts, close-window events and the final closed durable ledger. Its actual
native execution must be established by an exact-head macOS CI run.

The separately selected `conformance/softmax_app_views_focus` cases currently
exercise only action wire behavior on the Python reference SDK. They are not
proof of native effects, state/control reads, committed UI events, or passing the
same suite on both the native app and its mock. That integration remains open.

Catalog coverage remains incomplete: the existing census explicitly records six
unresolved window states. A green partial census/coverage ledger is not the named
full `softmax_app_catalog_coverage` acceptance. This foundation is not ready for
final full-PR review or landing until those gaps and required CI are resolved.
