package surface

import (
	"bytes"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/softmax-universe/coglet/sdk/surface-go/codec"
	"github.com/softmax-universe/coglet/sdk/surface-go/types"
)

const testSurface = "surface:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
const testCoglet = "coglet:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
const testOther = "coglet:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
const testID = "11111111111111111111111111111111"

type testHost struct {
	s       *Surface
	n       atomic.Uint64
	stops   atomic.Int64
	effects int
	ctx     RequestContext
	cfg     Config
}

func newHost(t *testing.T, async bool) *testHost {
	t.Helper()
	h := &testHost{}
	h.n.Store(100)
	dir := t.TempDir()
	if err := os.Chmod(dir, 0700); err != nil {
		t.Fatal(err)
	}
	h.cfg = Config{Identity: testSurface, Coglets: []string{testCoglet, testOther}, Ledger: filepath.Join(dir, "ledger"), Create: true, Clock: h.n.Load, Stop: func() { h.stops.Add(1) }}
	var err error
	h.s, err = New(h.cfg)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = h.s.Close() })
	op, _ := BoundOperation("expression.set_expression", nil, func(_ map[string]any, c RequestContext) (map[string]any, error) {
		h.effects++
		h.ctx = c
		if async {
			return nil, nil
		}
		return map[string]any{}, nil
	})
	must(t, h.s.Register(op))
	op, _ = BoundOperation("expression.state", nil, func(_ map[string]any, c RequestContext) (map[string]any, error) {
		return map[string]any{"expression": "neutral", "intensity": uint64(0), "remaining_ms": uint64(0)}, nil
	})
	must(t, h.s.Register(op))
	negotiate(t, h.s)
	must(t, h.s.Grant(h.grants()))
	h.s.Drain()
	return h
}
func (h *testHost) grants() []types.Grant {
	return []types.Grant{{Coglet: testCoglet, Operations: []string{"expression.set_expression", "expression.state"}, Resources: []string{"display.face"}, Expires: 10_000_000}}
}
func must(t *testing.T, e error) {
	t.Helper()
	if e != nil {
		t.Fatal(e)
	}
}
func negotiate(t *testing.T, s *Surface) {
	t.Helper()
	_, e := s.Open(testID)
	must(t, e)
	raw, e := message("hello_reply", map[string]any{"selection": map[string]any{"version": "0.1"}, "correlation_id": testID})
	must(t, e)
	must(t, s.Receive(raw))
	s.Drain()
}
func action(t *testing.T, id string, epoch, deadline uint64, args map[string]any) []byte {
	t.Helper()
	a := types.Action{Id: id, Coglet: testCoglet, Capability: "expression.set_expression", Args: args, Deadline: deadline, CorrelationId: testID}
	raw, e := message("act", types.Act{GrantEpoch: epoch, Action: a})
	must(t, e)
	return raw
}
func args() map[string]any {
	return map[string]any{"expression": "thinking", "intensity": uint64(5), "duration_ms": uint64(200)}
}
func messages(t *testing.T, s *Surface) []map[string]any {
	t.Helper()
	out := []map[string]any{}
	for _, r := range s.Drain() {
		m, e := decodeMessage(r)
		must(t, e)
		out = append(out, m)
	}
	return out
}
func body(m map[string]any) map[string]any { return m["body"].(map[string]any) }
func status(t *testing.T, s *Surface, want string) {
	t.Helper()
	ms := messages(t, s)
	if len(ms) != 1 || ms[0]["type"] != "event" || body(ms[0])["payload"].(map[string]any)["status"] != want {
		t.Fatalf("want terminal %s: %#v", want, ms)
	}
}
func TestNegotiationCapabilitiesGeneratedTypes(t *testing.T) {
	h := newHost(t, false)
	if h.s.Epoch() != 1 || !h.s.Ready() {
		t.Fatal("not ready")
	}
	must(t, h.s.Receive(action(t, testID, 1, 10000, args())))
	ms := messages(t, h.s)
	if len(ms) != 2 || body(ms[0])["accepted"] != true || body(ms[1])["payload"].(map[string]any)["status"] != "succeeded" || h.effects != 1 {
		t.Fatalf("bad delivery %#v", ms)
	}
}
func TestIdentityRetryAndConflict(t *testing.T) {
	h := newHost(t, false)
	raw := action(t, testID, 1, 10000, args())
	must(t, h.s.Receive(raw))
	first := messages(t, h.s)
	must(t, h.s.Receive(raw))
	again := messages(t, h.s)
	a, _ := codec.EncodeProfile(body(first[1])["payload"])
	b, _ := codec.EncodeProfile(body(again[1])["payload"])
	if !bytes.Equal(a, b) || h.effects != 1 || body(again[1])["seq"].(uint64) <= body(first[1])["seq"].(uint64) {
		t.Fatal("retry replayed effect or changed outcome/seq")
	}
	must(t, h.s.Receive(action(t, testID, 2, 10000, args())))
	ms := messages(t, h.s)
	if len(ms) != 1 || body(ms[0])["error"].(map[string]any)["code"] != "conflict" {
		t.Fatal(ms)
	}
}
func TestPendingDuplicateAndTerminalExactlyOnce(t *testing.T) {
	h := newHost(t, true)
	raw := action(t, testID, 1, 10000, args())
	must(t, h.s.Receive(raw))
	h.s.Drain()
	must(t, h.s.Receive(raw))
	if len(h.s.Drain()) != 1 || h.effects != 1 {
		t.Fatal("duplicate execution")
	}
	ok, e := h.s.Complete(h.ctx, "succeeded", map[string]any{}, "")
	must(t, e)
	if !ok {
		t.Fatal("no terminal")
	}
	status(t, h.s, "succeeded")
	ok, e = h.s.Complete(h.ctx, "succeeded", map[string]any{}, "")
	must(t, e)
	if ok || len(h.s.Drain()) != 0 {
		t.Fatal("duplicate terminal")
	}
}
func TestDeadlineExclusiveAtCompletionWithoutTick(t *testing.T) {
	for _, n := range []uint64{999, 1000, 1001} {
		t.Run(strconv.FormatUint(n, 10), func(t *testing.T) {
			h := newHost(t, true)
			must(t, h.s.Receive(action(t, testID, 1, 1000, args())))
			h.s.Drain()
			h.n.Store(n)
			ok, e := h.s.Complete(h.ctx, "succeeded", map[string]any{}, "")
			must(t, e)
			if !ok {
				t.Fatal("lost result")
			}
			want := "expired"
			if n < 1000 {
				want = "succeeded"
			}
			status(t, h.s, want)
		})
	}
}
func TestAdmissionFencesAndArgs(t *testing.T) {
	for _, tc := range []struct {
		name            string
		epoch, deadline uint64
		args            map[string]any
		code            string
	}{{"epoch", 2, 1000, args(), "permission_denied"}, {"equal", 1, 100, args(), "deadline_exceeded"}, {"future", 1, 60_000_101, args(), "invalid_argument"}, {"operation-schema", 1, 1000, map[string]any{"expression": "bogus", "intensity": 0, "duration_ms": 1}, "invalid_argument"}} {
		t.Run(tc.name, func(t *testing.T) {
			h := newHost(t, false)
			must(t, h.s.Receive(action(t, testID, tc.epoch, tc.deadline, tc.args)))
			ms := messages(t, h.s)
			if h.effects != 0 || len(ms) != 1 || body(ms[0])["error"].(map[string]any)["code"] != tc.code {
				t.Fatal(ms)
			}
		})
	}
}
func TestRevokeCancelsAndFencesOldContext(t *testing.T) {
	h := newHost(t, true)
	must(t, h.s.Receive(action(t, testID, 1, 10000, args())))
	h.s.Drain()
	old := h.ctx
	must(t, h.s.Grant(h.grants()))
	if old.Effect(func() error { t.Fatal("old effect ran"); return nil }) == nil {
		t.Fatal("old context authorized")
	}
	h.s.Drain()
	ok, e := h.s.Complete(old, "succeeded", map[string]any{}, "")
	must(t, e)
	if ok {
		t.Fatal("revoked completion won")
	}
	must(t, h.s.Receive(action(t, testID, 1, 10000, args())))
	ms := messages(t, h.s)
	if len(ms) != 2 || body(ms[1])["payload"].(map[string]any)["status"] != "cancelled" {
		t.Fatal(ms)
	}
}
func TestRestartRecoversUncertainWorkNeverReexecutes(t *testing.T) {
	h := newHost(t, true)
	raw := action(t, testID, 1, 10000, args())
	must(t, h.s.Receive(raw))
	h.s.Drain()
	h.s.ledger.close() // abrupt process loss: no graceful disconnect
	h.cfg.Create = false
	s, e := New(h.cfg)
	must(t, e)
	for _, op := range h.s.operations {
		must(t, s.Register(op.Operation))
	}
	h.s = s
	negotiate(t, s)
	must(t, s.Grant(h.grants()))
	s.Drain()
	must(t, s.Receive(raw))
	ms := messages(t, s)
	if len(ms) != 2 || body(ms[1])["payload"].(map[string]any)["status"] != "failed" || h.effects != 1 || s.Epoch() != 2 {
		t.Fatal(ms)
	}
}
func TestKillLatchedAcrossRestartAndLocalReset(t *testing.T) {
	h := newHost(t, true)
	must(t, h.s.Receive(action(t, testID, 1, 10000, args())))
	h.s.Drain()
	must(t, h.s.Kill("operator"))
	ms := messages(t, h.s)
	if len(ms) != 3 || ms[0]["type"] != "kill" || ms[1]["type"] != "health" || body(ms[1])["pending"] != uint64(0) {
		t.Fatal(ms)
	}
	must(t, h.s.Close())
	h.cfg.Create = false
	s, e := New(h.cfg)
	must(t, e)
	for _, op := range h.s.operations {
		must(t, s.Register(op.Operation))
	}
	h.s = s
	negotiate(t, s)
	must(t, s.Grant(h.grants()))
	s.Drain()
	if len(s.grants) != 0 {
		t.Fatal("kill restored grants")
	}
	if s.Reset() == nil {
		t.Fatal("wire-open reset")
	}
	must(t, s.Disconnect())
	must(t, s.Reset())
	negotiate(t, s)
	must(t, s.Grant(h.grants()))
	if len(s.grants) != 1 {
		t.Fatal("local reset failed")
	}
}
func TestUnsafeMissingCorruptAndLockedLedger(t *testing.T) {
	h := newHost(t, false)
	cfg := h.cfg
	cfg.Create = false
	if _, e := New(cfg); e == nil {
		t.Fatal("concurrent owner allowed")
	}
	must(t, h.s.Close())
	raw, e := os.ReadFile(cfg.Ledger)
	must(t, e)
	raw[len(raw)/2] ^= 1
	must(t, os.WriteFile(cfg.Ledger, raw, 0600))
	if _, e = New(cfg); e == nil {
		t.Fatal("corruption accepted")
	}
	must(t, os.Remove(cfg.Ledger))
	if _, e = New(cfg); e == nil {
		t.Fatal("lost ledger recreated")
	}
	must(t, os.Symlink("missing", cfg.Ledger))
	if _, e = New(cfg); e == nil {
		t.Fatal("symlink accepted")
	}
}
func TestClockAndDirectionFatal(t *testing.T) {
	h := newHost(t, true)
	must(t, h.s.Receive(action(t, testID, 1, 10000, args())))
	h.s.Drain()
	raw, e := message("grants", map[string]any{"epoch": uint64(2), "entries": []any{}, "correlation_id": testID})
	must(t, e)
	if h.s.Receive(raw) == nil || h.s.Ready() {
		t.Fatal("reverse grants accepted")
	}
	if h.stops.Load() < 2 {
		t.Fatal("no stop")
	}
	negotiate(t, h.s)
	h.n.Store(1)
	if h.s.Tick() == nil || h.s.Ready() {
		t.Fatal("rollback accepted")
	}
}
func TestReadSnapshotAndCorrelationRetention(t *testing.T) {
	h := newHost(t, false)
	r := types.Read{Coglet: testCoglet, Capability: "expression.state", Args: map[string]any{}, GrantEpoch: 1, Deadline: 10000, CorrelationId: testID}
	raw, e := message("read", r)
	must(t, e)
	must(t, h.s.Receive(raw))
	ms := messages(t, h.s)
	if len(ms) != 1 || body(ms[0])["snapshot"] != uint64(1) || body(ms[0])["ok"] != true {
		t.Fatal(ms)
	}
	if h.s.Receive(raw) == nil {
		t.Fatal("reused correlation accepted")
	}
}
func TestCatalogMatchesSpecAndSemanticByteBounds(t *testing.T) {
	want, e := os.ReadFile("../../spec/capabilities/bindings.json")
	must(t, e)
	if !bytes.Equal(want, catalogJSON) {
		t.Fatal("catalog drift")
	}
	if semantic("speech.speak_chunk", map[string]any{"text": strings.Repeat("é", 2049)}) == nil {
		t.Fatal("UTF8 bytes ignored")
	}
	if semantic("speech.work_result", map[string]any{"value": map[string]any{"text": strings.Repeat("x", 32000)}}) == nil {
		t.Fatal("CBOR overhead ignored")
	}
}
func TestRetentionAndBoundedQueue(t *testing.T) {
	h := newHost(t, false)
	raw := action(t, testID, 1, 1000, args())
	must(t, h.s.Receive(raw))
	h.s.Drain()
	h.n.Store(1000 + retention - 1)
	_, e := h.s.now()
	must(t, e)
	h.s.prune()
	if len(h.s.ledger.data.Actions) != 1 {
		t.Fatal("early eviction")
	}
	h.n.Store(1000 + retention)
	_, e = h.s.now()
	must(t, e)
	h.s.prune()
	if len(h.s.ledger.data.Actions) != 0 {
		t.Fatal("not pruned")
	}
	for i := 0; i < 128; i++ {
		must(t, h.s.health())
	}
	if h.s.health() == nil {
		t.Fatal("unbounded queue")
	}
}
func TestKillDoesNotWaitOnControlLock(t *testing.T) {
	h := newHost(t, false)
	stopped := make(chan struct{}, 1)
	h.s.stop = func() {
		select {
		case stopped <- struct{}{}:
		default:
		}
	}
	h.s.mu.Lock()
	done := make(chan error, 1)
	go func() { done <- h.s.Kill("operator") }()
	select {
	case <-stopped:
	case <-time.After(time.Second):
		h.s.mu.Unlock()
		t.Fatal("native reflex waited for control lock")
	}
	h.s.mu.Unlock()
	must(t, <-done)
}
