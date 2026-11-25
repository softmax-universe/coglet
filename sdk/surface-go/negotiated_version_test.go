package surface

import "testing"

func TestNegotiatedVersionConnectionLifetime(t *testing.T) {
	h := newPHost(t)
	r := newAppRig(t, h, true)
	if got := r.s.NegotiatedVersion(); got != "" {
		t.Fatalf("before negotiation: %q", got)
	}
	handshake(t, r.s, "0.2")
	if got := r.s.NegotiatedVersion(); got != "0.2" {
		t.Fatalf("selected version: %q", got)
	}
	// A fatal invalid frame clears the selected version, not provisioning.
	if err := r.s.Receive([]byte{0xff}); err == nil {
		t.Fatal("invalid frame accepted")
	}
	if got := r.s.NegotiatedVersion(); got != "" {
		t.Fatalf("disconnected version: %q", got)
	}
	handshake(t, r.s, "0.1")
	if got := r.s.NegotiatedVersion(); got != "0.1" {
		t.Fatalf("downgraded reconnect: %q", got)
	}
}
