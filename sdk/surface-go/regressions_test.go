package surface

import (
	"errors"
	"fmt"
	"path/filepath"
	"sync/atomic"
	"testing"

	"github.com/softmax-universe/coglet/sdk/surface-go/types"
)

func TestKillRetainsReceiptsAtReservedCapacity(t *testing.T) {
	for _, async := range []bool{false, true} {
		t.Run(fmt.Sprint(async), func(t *testing.T) {
			h := newHost(t, async)
			for i := 0; i < 63; i++ {
				must(t, h.s.Receive(action(t, fmt.Sprintf("%032x", i+1), 1, 10000, args())))
			}
			if len(h.s.queue)+h.s.pending() != 126 {
				t.Fatal("terminal reservation mismatch")
			}
			if h.s.health() == nil {
				t.Fatal("normal frame consumed kill/terminal reserve")
			}
			must(t, h.s.Kill("operator"))
			ms := messages(t, h.s)
			if len(ms) != 128 || ms[0]["type"] != "kill" || ms[1]["type"] != "health" || body(ms[1])["pending"] != uint64(0) || !h.s.Ready() {
				t.Fatal("kill lost reserved notifications/receipts", len(ms))
			}
			accepts, outcomes := 0, 0
			for _, m := range ms[2:] {
				switch m["type"] {
				case "accept":
					accepts++
				case "event":
					outcomes++
				}
			}
			if accepts != 63 || outcomes != 63 {
				t.Fatal("admission/terminal deliveries dropped", accepts, outcomes)
			}
		})
	}
}

func TestInvalidGrantSnapshotIsAtomic(t *testing.T) {
	h := newHost(t, false)
	epoch := h.s.Epoch()
	gen := h.s.generation.Load()
	g := h.grants()
	g = append(g, types.Grant{Coglet: testOther, Operations: []string{"expression.state"}, Resources: []string{"display.face"}, Expires: 10000})
	if h.s.Grant(g) == nil {
		t.Fatal("duplicate resource accepted")
	}
	if !h.s.Ready() || h.s.Epoch() != epoch || h.s.generation.Load() != gen || len(h.s.grants) != 1 {
		t.Fatal("invalid snapshot changed authority")
	}
}
func TestPrivateEventRevocationAndStaleSender(t *testing.T) {
	h := newHost(t, false)
	must(t, h.s.Disconnect())
	op, _ := BoundOperation("speech.partial", nil, nil)
	must(t, h.s.Register(op))
	negotiate(t, h.s)
	g := h.grants()
	g[0].Operations = append(g[0].Operations, "speech.partial")
	g[0].Resources = append(g[0].Resources, "audio.mic", "audio.speaker")
	must(t, h.s.Grant(g))
	h.s.Drain()
	ctx, e := h.s.Context(testCoglet)
	must(t, e)
	payload := map[string]any{"utterance": testID, "revision": uint64(1), "text": "private"}
	must(t, h.s.Event(ctx, "speech.partial", payload))
	if len(h.s.queue) != 1 {
		t.Fatal("event not queued")
	}
	must(t, h.s.Grant(g))
	for _, m := range messages(t, h.s) {
		if m["type"] == "event" {
			t.Fatal("private queued frame survived revoke")
		}
	}
	if h.s.Event(ctx, "speech.partial", payload) == nil {
		t.Fatal("stale sender accepted")
	}
	if !h.s.Ready() {
		t.Fatal("stale callback disconnected new owner")
	}
	fresh, e := h.s.Context(testCoglet)
	must(t, e)
	must(t, h.s.Event(fresh, "speech.partial", payload))
	if len(h.s.Drain()) != 1 {
		t.Fatal("new sender failed")
	}
}
func TestDurableAdmissionFailureCannotStartEffect(t *testing.T) {
	h := newHost(t, false)
	old := h.s.ledger.path
	h.s.ledger.path = filepath.Join(filepath.Dir(old), "missing", "ledger")
	err := h.s.Receive(action(t, testID, 1, 10000, args()))
	h.s.ledger.path = old
	if err == nil || h.effects != 0 || h.s.Ready() || len(h.s.Drain()) != 0 {
		t.Fatal("failed persistence acknowledged or started work")
	}
}
func TestFinalEffectFenceAfterDurableAdmission(t *testing.T) {
	h := newHost(t, false)
	var samples atomic.Uint64
	h.s.clock = func() uint64 {
		if samples.Add(1) >= 4 {
			return 1000
		}
		return 100
	}
	must(t, h.s.Receive(action(t, testID, 1, 1000, args())))
	ms := messages(t, h.s)
	if h.effects != 0 || len(ms) != 2 || body(ms[0])["accepted"] != true || body(ms[1])["payload"].(map[string]any)["status"] != "expired" {
		t.Fatal("post-persistence expiry started effect", ms)
	}
}
func TestUncleanRestartRemainsLatchedEvenWithoutPendingActions(t *testing.T) {
	h := newHost(t, false)
	h.s.ledger.close()
	h.cfg.Create = false
	s, e := New(h.cfg)
	must(t, e)
	h.s = s
	if !s.killed.Load() {
		t.Fatal("unclean running marker ignored")
	}
	negotiate(t, s)
	must(t, s.Grant(nil))
	if len(s.grants) != 0 {
		t.Fatal("unclean restart restored authority")
	}
}
func TestReadAndHealthTimeoutCancelPending(t *testing.T) {
	h := newHost(t, true)
	must(t, h.s.Receive(action(t, testID, 1, 4_000_000, args())))
	h.s.Drain()
	h.n.Store(3_000_100)
	if h.s.Tick() == nil || h.s.Ready() {
		t.Fatal("peer timeout ignored")
	}
	r := h.s.ledger.data.Actions[testCoglet+"/"+testID]
	if r.Outcome == nil {
		t.Fatal("timeout lost pending terminal")
	}
}
func TestPrepareCannotReenterNativeEffectAfterRevoke(t *testing.T) {
	h := newHost(t, true)
	must(t, h.s.Receive(action(t, testID, 1, 10000, args())))
	h.s.Drain()
	old := h.ctx
	must(t, h.s.Disconnect())
	if !errors.As(old.Effect(func() error { t.Fatal("disconnected effect ran"); return nil }), new(*Rejection)) {
		t.Fatal("missing cancellation fence")
	}
}
