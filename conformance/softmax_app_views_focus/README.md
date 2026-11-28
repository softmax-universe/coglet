# softmax.app views/focus product conformance (ADR-0030, surface-0.2)

Separately selected, product-local suite `softmax_app_views_focus`. It touches no shared
harness, workflow, registry or frozen `surface`/`speech`/`surface02` case. Every `*.json` here is
an executable harness case (harness plays the **runtime**, the implementation is a **surface**,
real stdio CBOR frames); non-case data uses other suffixes (`adapter-config.fixture`).

```sh
# from the repo root (uv supplies cbor2 so the child `python3` can import the SDK)
uv run --with cbor2 --with jsonschema python3 conformance/run \
  --registry conformance/softmax_app_views_focus/impls.toml \
  --contract-profile surface-0.2 --suite softmax_app_views_focus
uv run --with cbor2 --with jsonschema python3 -m unittest discover \
  -s conformance/softmax_app_views_focus -p 'test_*.py' -v
```

## What is covered (Linux, executed)
Operations `navigate`, `open_window`, `close_window`, `set_focus`, against the **real pinned
app catalog** `apps/softmax-mac/Surface/catalog.v1.json` (sha256 `3b2ad502…54f9`, checked by test):
0.2 selection; `softmax.app` entry advertisement with exclusive `softmax.app_control`; each action
accepted with a `succeeded` outcome; closed-schema refusals (mismatched window/route, extra field,
unknown window, missing/empty/oversize/null control) as `invalid_argument` followed by a valid
action as the very next accept; ungranted `invoke`/`invoke_sensitive`/`set_setting`, a
two-component alias and a prefix name as `permission_denied`; wrong grant epoch refused;
exact retry replays acceptance and outcome. Mutating an expectation makes the case fail (checked).

## Limits — read before citing this as proof
- The Linux implementation is the **Python reference SDK surface** provisioned with the app
  catalog. It validates the wire and schemas for real but is **not the Softmax app**: its
  actions have no native effect. Passing here is **not native, app, UI or macOS proof**.
- `state`, `controls` (reads) and `view_changed`, `focus_changed`, `window_changed` (events) are
  **not covered**: the reference returns a fixed non-catalog read value and cannot emit
  committed-UI events. A passing run must not be read as covering them.
- Not covered: kill, restart, deadlines, owner replacement, sensitive controls with a granted
  sensitive resource. Those need the app.
- Gap for the app: it listens on a private DEBUG-only Unix socket driven by
  `AppSurfaceWireProof` with the Rust runtime peer `crates/coglet-mock/examples/app_surface_peer.rs`,
  which expects stdin commands and a UI-driven timeline. The fixed-timeline harness (`--socket`)
  has no way to cause or await UI changes, so reads/events need an app-side driver (a
  native-CI step feeding `app_surface_peer` commands and diffing the printed frames), or a
  harness extension for fixture-triggered native mutations. Neither is added here; no native
  registry entry is claimed.
