package surface

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/softmax-universe/coglet/sdk/surface-go/types"
)

// Synthetic product-neutral catalog using the narrow softmax.app.<component> names.
const appCatalog = `{
 "version": 1,
 "namespace": "softmax",
 "entries": {"softmax.app": ["softmax.app_ctl"], "softmax.app_sensitive": ["softmax.app_secret"]},
 "schemas": {
  "https://example.invalid/app/1#empty": "empty",
  "https://example.invalid/app/1#state": "state",
  "https://example.invalid/app/1#open": "open"
 },
 "operations": {
  "softmax.app.status": {"entry":"softmax.app","direction":"read","input":"https://example.invalid/app/1#empty","output":"https://example.invalid/app/1#state","resources":[],"single_owner":true},
  "softmax.app.open": {"entry":"softmax.app","direction":"action","input":"https://example.invalid/app/1#open","output":"https://example.invalid/app/1#empty","resources":["softmax.app_ctl"],"single_owner":true},
  "softmax.app.changed": {"entry":"softmax.app","direction":"event","input":"https://example.invalid/app/1#state","resources":[],"single_owner":true}
 },
 "rules": {
  "empty": {"op":"record","fields":[]},
  "state": {"op":"record","fields":[{"name":"paused","optional":false,"type":{"op":"primitive","name":"bool"}}]},
  "open": {"op":"record","fields":[{"name":"target","optional":false,"type":{"op":"range","min":0,"max":9}}]}
 }
}`

func pin(raw string) string {
	sum := sha256.Sum256([]byte(raw))
	return hex.EncodeToString(sum[:])
}
func mustCatalog(t *testing.T, raw string) *TrustedCatalog {
	t.Helper()
	c, err := NewTrustedCatalog([]byte(raw), pin(raw))
	if err != nil {
		t.Fatal(err)
	}
	return c
}

type pHost struct {
	dir   string
	clock atomic.Uint64
	stops atomic.Int64
}

func newPHost(t *testing.T) *pHost {
	t.Helper()
	h := &pHost{dir: t.TempDir()}
	if err := os.Chmod(h.dir, 0700); err != nil {
		t.Fatal(err)
	}
	h.clock.Store(100)
	return h
}
func (h *pHost) cfg(profile string, cat *TrustedCatalog, create bool) Config {
	return Config{Identity: testSurface, Coglets: []string{testCoglet}, Ledger: filepath.Join(h.dir, "ledger"), Create: create,
		Clock: h.clock.Load, Stop: func() { h.stops.Add(1) }, ContractProfile: profile, TrustedCatalog: cat}
}
func (h *pHost) open(t *testing.T, profile string, cat *TrustedCatalog, create bool) *Surface {
	t.Helper()
	s, err := New(h.cfg(profile, cat, create))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

// ledgerMap reads the sealed ledger data for tampering/inspection.
func ledgerMap(t *testing.T, path string) map[string]any {
	t.Helper()
	raw, err := os.ReadFile(path)
	must(t, err)
	var s sealed
	must(t, json.Unmarshal(raw, &s))
	var m map[string]any
	must(t, json.Unmarshal(s.Data, &m))
	return m
}
func writeLedgerMap(t *testing.T, path string, m map[string]any) {
	t.Helper()
	data, err := json.Marshal(m)
	must(t, err)
	sum := sha256.Sum256(data)
	raw, err := json.Marshal(sealed{data, hex.EncodeToString(sum[:])})
	must(t, err)
	must(t, os.WriteFile(path, raw, 0600))
}

func offered(t *testing.T, raw []byte) []any {
	t.Helper()
	m, err := decodeMessage(raw)
	must(t, err)
	return body(m)["offer"].(map[string]any)["versions"].([]any)
}
func handshake(t *testing.T, s *Surface, version string) []map[string]any {
	t.Helper()
	_, e := s.Open(testID)
	must(t, e)
	raw, e := encodeWith(anyWireSchema(), "hello_reply", map[string]any{"selection": map[string]any{"version": version}, "correlation_id": testID})
	must(t, e)
	must(t, s.Receive(raw))
	return messages(t, s)
}
func capNames(m map[string]any) map[string]map[string]any {
	out := map[string]map[string]any{}
	for _, e := range body(m)["entries"].([]any) {
		em := e.(map[string]any)
		out[em["name"].(string)] = em
	}
	return out
}

func TestLocalContractProfileDefaultsAndUnknowns(t *testing.T) {
	for _, p := range []string{"", ProfileSurface01} {
		h := newPHost(t)
		s := h.open(t, p, nil, true)
		raw, err := s.Open(testID)
		must(t, err)
		if v := offered(t, raw); len(v) != 1 || v[0] != "0.1" {
			t.Fatalf("profile %q must offer only 0.1: %v", p, v)
		}
		if m := ledgerMap(t, filepath.Join(h.dir, "ledger")); m["contract_profile"] != ProfileSurface01 {
			t.Fatalf("omitted/explicit 0.1 must persist the same normalized identity: %v", m["contract_profile"])
		}
		// 0.1 never accepts a 0.2 selection.
		bad, _ := encodeWith(anyWireSchema(), "hello_reply", map[string]any{"selection": map[string]any{"version": "0.2"}, "correlation_id": testID})
		if s.Receive(bad) == nil {
			t.Fatal("surface-0.1 accepted 0.2 selection")
		}
	}
	for _, bad := range []string{"surface-0.3", "Surface-0.2", " surface-0.1", "surface-0.2 ", "0.2", "0.1", "surface-0.10", "surface-0.2\n", "softmax", "surface-0.0"} {
		h := newPHost(t)
		if _, err := New(h.cfg(bad, nil, true)); err == nil {
			t.Fatalf("unknown profile %q accepted", bad)
		}
		if _, err := os.Stat(filepath.Join(h.dir, "ledger")); err == nil {
			t.Fatalf("refused profile %q left a ledger", bad)
		}
	}
	h := newPHost(t)
	s := h.open(t, ProfileSurface02, nil, true)
	raw, err := s.Open(testID)
	must(t, err)
	if v := offered(t, raw); len(v) != 2 || v[0] != "0.2" || v[1] != "0.1" {
		t.Fatalf("surface-0.2 offers 0.2 then 0.1: %v", v)
	}
	// A peer cannot select a version that was never offered.
	nope, _ := encodeWith(anyWireSchema(), "hello_reply", map[string]any{"selection": map[string]any{"version": "0.3"}, "correlation_id": testID})
	if s.Receive(nope) == nil {
		t.Fatal("unoffered version selected")
	}
	// Existing config literals stay valid and unchanged: frozen 0.1 default.
	s2 := newHost(t, false)
	if s2.s.profile != ProfileSurface01 {
		t.Fatalf("default profile %q", s2.s.profile)
	}
}

func TestLocalContractProfileCatalogV1Closed(t *testing.T) {
	good := appCatalog
	mut := func(old, repl string) string {
		if !strings.Contains(good, old) {
			t.Fatalf("fixture lacks %q", old)
		}
		return strings.Replace(good, old, repl, 1)
	}
	cases := map[string]string{
		"contract_profile member": mut(`"version": 1,`, `"version": 1, "contract_profile": "surface-0.2",`),
		"unknown top field":       mut(`"version": 1,`, `"version": 1, "extra": 1,`),
		"unknown operation field": mut(`"single_owner":true},
  "softmax.app.open"`, `"single_owner":true,"x":1},
  "softmax.app.open"`),
		"version 2":           mut(`"version": 1`, `"version": 2`),
		"version 1.0":         mut(`"version": 1`, `"version": 1.0`),
		"duplicate key":       mut(`"namespace": "softmax",`, `"namespace": "softmax", "namespace": "softmax",`),
		"BOM":                 "\xef\xbb\xbf" + good,
		"trailing token":      good + " {}",
		"reserved namespace":  strings.ReplaceAll(good, "softmax", "speech"),
		"open map IR":         mut(`"empty": {"op":"record","fields":[]}`, `"empty": {"op":"map","min":0,"item":{"op":"primitive","name":"tstr"}}`),
		"unbounded array":     mut(`"empty": {"op":"record","fields":[]}`, `"empty": {"op":"array","min":0,"max":999999,"item":{"op":"primitive","name":"bool"}}`),
		"recursive rule":      mut(`"empty": {"op":"record","fields":[]}`, `"empty": {"op":"record","fields":[{"name":"x","optional":false,"type":{"op":"ref","name":"empty"}}]}`),
		"unused bad rule":     mut(`"empty": {"op":"record","fields":[]}`, `"empty": {"op":"record","fields":[]}, "unused": {"op":"bogus"}`),
		"unknown ref":         mut(`"empty": {"op":"record","fields":[]}`, `"empty": {"op":"ref","name":"nowhere"}`),
		"undeclared resource": mut(`"resources":["softmax.app_ctl"]`, `"resources":["softmax.app_secret"]`),
		"missing output":      mut(`"output":"https://example.invalid/app/1#state",`, ``),
		"event output":        mut(`"direction":"event","input":"https://example.invalid/app/1#state",`, `"direction":"event","output":"https://example.invalid/app/1#state","input":"https://example.invalid/app/1#state",`),
		"reserved schema URI": mut(`https://example.invalid/app/1#state`, `https://coglet.dev/spec/messages/x#state`),
		"four component name": mut(`softmax.app.changed`, `softmax.app.x.changed`),
		"prefix-only name":    mut(`"softmax.app.changed"`, `"softmax.apps.changed"`),
		"not single owner": mut(`"single_owner":true},
  "softmax.app.open"`, `"single_owner":false},
  "softmax.app.open"`),
		"built-in override": mut(`"softmax.app.changed"`, `"expression.state"`),
	}
	for name, raw := range cases {
		if _, err := NewTrustedCatalog([]byte(raw), pin(raw)); err == nil {
			t.Errorf("%s: closed v1 artifact accepted", name)
		}
	}
	// Wrong, uppercase, short and empty pins fail before any parsing.
	for _, p := range []string{pin(good + " "), strings.ToUpper(pin(good)), pin(good)[:63], ""} {
		if _, err := NewTrustedCatalog([]byte(good), p); err == nil {
			t.Errorf("pin %q accepted", p)
		}
	}
	// Only the constructor yields a usable catalog.
	h := newPHost(t)
	if _, err := New(h.cfg(ProfileSurface02, &TrustedCatalog{}, true)); err == nil {
		t.Fatal("zero-value catalog accepted")
	}
	// Names require the 0.2 profile; omitted/0.1 refuses app names at New.
	c := mustCatalog(t, good)
	for _, p := range []string{"", ProfileSurface01} {
		if _, err := New(newPHost(t).cfg(p, c, true)); err == nil {
			t.Fatalf("app catalog accepted under profile %q", p)
		}
	}
	if c.Digest() != pin(good) {
		t.Fatal("digest")
	}
}

func TestLocalContractProfileReplayPairFenced(t *testing.T) {
	cat := mustCatalog(t, appCatalog)
	other := strings.Replace(appCatalog, `"max":9`, `"max":8`, 1)
	otherCat := mustCatalog(t, other)
	path := func(h *pHost) string { return filepath.Join(h.dir, "ledger") }

	h := newPHost(t)
	s := h.open(t, ProfileSurface02, cat, true)
	// Pair is durable before the service is exposed (no Open yet).
	m := ledgerMap(t, path(h))
	if m["contract_profile"] != ProfileSurface02 || m["CatalogDigest"] != cat.Digest() {
		t.Fatalf("pair not persisted at creation: %v", m)
	}
	must(t, s.Close())
	before, _ := os.ReadFile(path(h))
	refuse := func(why string, p string, c *TrustedCatalog) {
		t.Helper()
		if s2, err := New(h.cfg(p, c, false)); err == nil {
			_ = s2.Close()
			t.Fatalf("%s: recovery accepted", why)
		}
		after, _ := os.ReadFile(path(h))
		if string(after) != string(before) {
			t.Fatalf("%s: refused recovery mutated the ledger", why)
		}
	}
	refuse("same digest, other profile", ProfileSurface01, cat)
	refuse("omitted profile", "", cat)
	refuse("removed catalog", ProfileSurface02, nil)
	refuse("changed catalog", ProfileSurface02, otherCat)
	reopened, err := New(h.cfg(ProfileSurface02, cat, false))
	must(t, err)
	must(t, reopened.Close())

	// No-catalog state retains an absent digest plus its profile.
	h2 := newPHost(t)
	s = h2.open(t, ProfileSurface02, nil, true)
	m = ledgerMap(t, path(h2))
	if _, has := m["CatalogDigest"]; has || m["contract_profile"] != ProfileSurface02 {
		t.Fatalf("catalog-free pair: %v", m)
	}
	must(t, s.Close())
	if s3, err := New(h2.cfg(ProfileSurface02, cat, false)); err == nil {
		_ = s3.Close()
		t.Fatal("added catalog accepted on catalog-free ledger")
	}
	if s3, err := New(h2.cfg(ProfileSurface01, nil, false)); err == nil {
		_ = s3.Close()
		t.Fatal("0.2 ledger reopened as 0.1")
	}

	// Legacy profile-less ledger means exactly surface-0.1: it cannot be
	// reinterpreted as 0.2, even without a catalog; unknown saved profiles fail.
	h3 := newPHost(t)
	s = h3.open(t, "", nil, true)
	must(t, s.Close())
	m = ledgerMap(t, path(h3))
	delete(m, "contract_profile")
	writeLedgerMap(t, path(h3), m)
	if s3, err := New(h3.cfg(ProfileSurface02, nil, false)); err == nil {
		_ = s3.Close()
		t.Fatal("legacy ledger reinterpreted as surface-0.2")
	}
	if s3, err := New(h3.cfg(ProfileSurface02, cat, false)); err == nil {
		_ = s3.Close()
		t.Fatal("legacy ledger adopted catalog")
	}
	legacy, err := New(h3.cfg("", nil, false))
	must(t, err)
	must(t, legacy.Close())
	if got := ledgerMap(t, path(h3))["contract_profile"]; got != ProfileSurface01 {
		t.Fatalf("legacy normalized to %v", got)
	}
	m = ledgerMap(t, path(h3))
	m["contract_profile"] = "surface-0.9"
	writeLedgerMap(t, path(h3), m)
	for _, p := range []string{"", ProfileSurface01, ProfileSurface02} {
		if s3, err := New(h3.cfg(p, nil, false)); err == nil {
			_ = s3.Close()
			t.Fatalf("unknown saved profile accepted under %q", p)
		}
	}
}

type appRig struct {
	s        *Surface
	exec     int
	statusN  int
	statusOK bool
}

func newAppRig(t *testing.T, h *pHost, create bool) *appRig {
	t.Helper()
	r := &appRig{statusOK: true}
	r.s = h.open(t, ProfileSurface02, mustCatalog(t, appCatalog), create)
	face, _ := r.s.BoundOperation("expression.set_expression", nil, func(map[string]any, RequestContext) (map[string]any, error) { r.exec++; return map[string]any{}, nil })
	must(t, r.s.Register(face))
	open, err := r.s.BoundOperation("softmax.app.open", nil, func(map[string]any, RequestContext) (map[string]any, error) { r.exec++; return map[string]any{}, nil })
	must(t, err)
	must(t, r.s.Register(open))
	status, err := r.s.BoundOperation("softmax.app.status", nil, func(map[string]any, RequestContext) (map[string]any, error) {
		r.statusN++
		if r.statusOK {
			return map[string]any{"paused": false}, nil
		}
		return map[string]any{"paused": "no"}, nil
	})
	must(t, err)
	must(t, r.s.Register(status))
	changed, err := r.s.BoundOperation("softmax.app.changed", nil, nil)
	must(t, err)
	must(t, r.s.Register(changed))
	if _, err = r.s.BoundOperation("softmax.app.missing", nil, nil); err == nil {
		t.Fatal("unknown catalog operation bound")
	}
	return r
}
func appGrant() []types.Grant {
	return []types.Grant{{Coglet: testCoglet, Operations: []string{"expression.set_expression", "softmax.app.open", "softmax.app.status", "softmax.app.changed"},
		Resources: []string{"display.face", "softmax.app_ctl"}, Expires: 10_000_000}}
}
func sendAct(t *testing.T, s *Surface, capability, id string, epoch uint64, args map[string]any) error {
	t.Helper()
	a := types.Action{Id: id, Coglet: testCoglet, Capability: capability, Args: args, Deadline: 5_000_000, CorrelationId: testID}
	raw, e := encodeWith(anyWireSchema(), "act", types.Act{GrantEpoch: epoch, Action: a})
	must(t, e)
	return s.Receive(raw)
}
func sendRead(t *testing.T, s *Surface, capability, cid string, epoch uint64, args map[string]any) error {
	t.Helper()
	raw, e := encodeWith(anyWireSchema(), "read", map[string]any{"coglet": testCoglet, "capability": capability, "args": args, "grant_epoch": epoch, "deadline": uint64(5_000_000), "correlation_id": cid})
	must(t, e)
	return s.Receive(raw)
}
func acceptCode(m map[string]any) string {
	b := body(m)
	if b["accepted"] == true {
		return ""
	}
	return b["error"].(map[string]any)["code"].(string)
}

func TestLocalContractProfileSelectedValidationBeforeDispatch(t *testing.T) {
	h := newPHost(t)
	r := newAppRig(t, h, true)
	ms := handshake(t, r.s, "0.2")
	caps := capNames(ms[0])
	app := caps["softmax.app"]
	if app == nil || fmt.Sprint(app["actions"]) != "[softmax.app.open]" || fmt.Sprint(app["reads"]) != "[softmax.app.status]" || fmt.Sprint(app["events"]) != "[softmax.app.changed]" {
		t.Fatalf("0.2 capabilities: %v", caps)
	}
	if app["schemas"].(map[string]any)["softmax.app.open"] != "https://example.invalid/app/1#open" ||
		app["schemas"].(map[string]any)["softmax.app.status"] != "https://example.invalid/app/1#state" {
		t.Fatalf("exact advertised schema URIs: %v", app["schemas"])
	}
	if caps["expression.control"] == nil {
		t.Fatal("Face lost at 0.2")
	}
	must(t, r.s.Grant(appGrant()))
	epoch := r.s.Epoch()
	r.s.Drain()
	// Positive control.
	must(t, sendAct(t, r.s, "softmax.app.open", "a0000000000000000000000000000001", epoch, map[string]any{"target": uint64(3)}))
	if ms = messages(t, r.s); acceptCode(ms[0]) != "" || r.exec != 1 {
		t.Fatalf("positive app action: %v exec=%d", ms, r.exec)
	}
	// Invalid payloads, sibling payloads and unknown names fail before dispatch.
	for i, args := range []map[string]any{{"target": uint64(10)}, {"target": "x"}, {"paused": true}, {}, {"target": uint64(1), "extra": true}} {
		must(t, sendAct(t, r.s, "softmax.app.open", fmt.Sprintf("b000000000000000000000000000000%d", i), epoch, args))
		if ms = messages(t, r.s); acceptCode(ms[0]) != "invalid_argument" {
			t.Fatalf("args %v admitted: %v", args, ms)
		}
	}
	must(t, sendAct(t, r.s, "softmax.app.unknown", "c0000000000000000000000000000001", epoch, map[string]any{}))
	if ms = messages(t, r.s); acceptCode(ms[0]) == "" {
		t.Fatalf("unknown app name admitted: %v", ms)
	}
	if r.exec != 1 {
		t.Fatalf("invalid payload reached dispatch: %d", r.exec)
	}
	// Reads validate input and output; the backend callback cannot widen output.
	must(t, sendRead(t, r.s, "softmax.app.status", "d0000000000000000000000000000001", epoch, map[string]any{}))
	if ms = messages(t, r.s); body(ms[0])["ok"] != true {
		t.Fatalf("status read: %v", ms)
	}
	r.statusOK = false
	must(t, sendRead(t, r.s, "softmax.app.status", "d0000000000000000000000000000002", epoch, map[string]any{}))
	if ms = messages(t, r.s); body(ms[0])["ok"] != false {
		t.Fatalf("invalid backend output returned: %v", ms)
	}
	must(t, sendRead(t, r.s, "softmax.app.status", "d0000000000000000000000000000003", epoch, map[string]any{"x": true}))
	if ms = messages(t, r.s); body(ms[0])["ok"] != false {
		t.Fatalf("invalid read args accepted: %v", ms)
	}
	// Event: selected input schema, then retained 0.2 grants.
	ctx, err := r.s.Context(testCoglet)
	must(t, err)
	if err = r.s.Event(ctx, "softmax.app.changed", map[string]any{"paused": "x"}); err == nil {
		t.Fatal("invalid event payload published")
	}
	r.s.Drain()
	h.clock.Add(1)
	ctx, _ = r.s.Context(testCoglet)
	must(t, r.s.Event(ctx, "softmax.app.changed", map[string]any{"paused": true}))
	if ms = messages(t, r.s); len(ms) != 1 || body(ms[0])["kind"] != "softmax.app.changed" {
		t.Fatalf("app event: %v", ms)
	}
	// Frozen names still validate under 0.2, other three-component names do not.
	must(t, sendAct(t, r.s, "expression.set_expression", "e0000000000000000000000000000001", epoch, args()))
	if ms = messages(t, r.s); acceptCode(ms[0]) != "" {
		t.Fatalf("0.1-name action refused at 0.2: %v", ms)
	}
	// Same-length non-app name (softmax.bpp.open), crafted by byte substitution.
	a := types.Action{Id: "e0000000000000000000000000000002", Coglet: testCoglet, Capability: "softmax.app.open", Args: map[string]any{"target": uint64(1)}, Deadline: 5_000_000, CorrelationId: testID}
	raw, e := encodeWith(anyWireSchema(), "act", types.Act{GrantEpoch: epoch, Action: a})
	must(t, e)
	if err = r.s.Receive(bytes.Replace(raw, []byte("softmax.app.open"), []byte("softmax.bpp.open"), 1)); err == nil {
		t.Fatal("non-app three-component name decoded at 0.2")
	}
}

func TestLocalContractProfileDowngradeFaceSpeech(t *testing.T) {
	h := newPHost(t)
	r := newAppRig(t, h, true)
	ms := handshake(t, r.s, "0.1")
	caps := capNames(ms[0])
	if len(caps) != 1 || caps["expression.control"] == nil {
		t.Fatalf("downgrade advertises only built-ins: %v", caps)
	}
	for _, e := range caps {
		for _, k := range []string{"events", "reads", "actions", "exclusive"} {
			if strings.Contains(fmt.Sprint(e[k]), "softmax") {
				t.Fatalf("app operation/resource advertised at 0.1: %v", e)
			}
		}
		for n := range e["schemas"].(map[string]any) {
			if strings.HasPrefix(n, "softmax.") {
				t.Fatalf("app schema advertised at 0.1: %v", n)
			}
		}
	}
	// A saved/stale grant naming app operations cannot be restored at 0.1.
	if err := r.s.Grant(appGrant()); err == nil {
		t.Fatal("app grant accepted on downgraded connection")
	}
	r.s.Drain()
	face := []types.Grant{{Coglet: testCoglet, Operations: []string{"expression.set_expression"}, Resources: []string{"display.face"}, Expires: 10_000_000}}
	must(t, r.s.Grant(face))
	epoch := r.s.Epoch()
	r.s.Drain()
	// Face keeps working unchanged.
	must(t, sendAct(t, r.s, "expression.set_expression", "f0000000000000000000000000000001", epoch, args()))
	if ms = messages(t, r.s); len(ms) < 1 || acceptCode(ms[0]) != "" || r.exec != 1 {
		t.Fatalf("Face action at 0.1: %v exec=%d", ms, r.exec)
	}
	// App actions are not decodable at 0.1: fatal fail-closed, never dispatched.
	if err := sendAct(t, r.s, "softmax.app.open", "f0000000000000000000000000000002", epoch, map[string]any{"target": uint64(1)}); err == nil {
		t.Fatal("0.2-only name decoded on a 0.1 connection")
	}
	if r.exec != 1 || r.s.Ready() {
		t.Fatalf("app action reached dispatch or kept session: exec=%d ready=%v", r.exec, r.s.Ready())
	}
	// Reconnect at 0.1: reads and events are unavailable as well.
	handshake(t, r.s, "0.1")
	must(t, r.s.Grant(face))
	epoch = r.s.Epoch()
	r.s.Drain()
	if err := sendRead(t, r.s, "softmax.app.status", "f1000000000000000000000000000001", epoch, map[string]any{}); err == nil || r.statusN != 0 || r.s.Ready() {
		t.Fatalf("app read decoded/served at 0.1: %v n=%d", err, r.statusN)
	}
	handshake(t, r.s, "0.1")
	must(t, r.s.Grant(face))
	r.s.Drain()
	ctx, err := r.s.Context(testCoglet)
	must(t, err)
	if err = r.s.Event(ctx, "softmax.app.changed", map[string]any{"paused": true}); err == nil {
		t.Fatal("app event published at 0.1")
	}
	if q := r.s.Drain(); len(q) != 0 {
		t.Fatalf("queued app publication at 0.1: %d", len(q))
	}
	// Catalog and surface-0.2 identity are retained, not erased or relabeled.
	if m := ledgerMap(t, filepath.Join(h.dir, "ledger")); m["contract_profile"] != ProfileSurface02 || m["CatalogDigest"] != pin(appCatalog) {
		t.Fatalf("downgrade rewrote stored identity: %v", m)
	}
	must(t, r.s.Close())
	// Reconnect at 0.2 requires fresh grants and epoch; nothing is resurrected.
	r2 := newAppRig(t, h, false)
	ms = handshake(t, r2.s, "0.2")
	if capNames(ms[0])["softmax.app"] == nil {
		t.Fatal("app catalog not restored on 0.2 reconnect")
	}
	if err = sendAct(t, r2.s, "softmax.app.open", "f2000000000000000000000000000001", r.s.Epoch(), map[string]any{"target": uint64(1)}); err != nil {
		t.Fatal(err)
	}
	if ms = messages(t, r2.s); acceptCode(ms[0]) != "permission_denied" || r2.exec != 0 {
		t.Fatalf("old grant resurrected at 0.2: %v", ms)
	}
}
