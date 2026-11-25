package surface

import (
	"github.com/softmax-universe/coglet/sdk/surface-go/codec"
	"testing"
)

// The profile consumes the published generated family; strict roots must remain
// frozen even after constructing a negotiated-0.2 validator.
func TestGeneratedSurface02Selection(t *testing.T) {
	strict, err := buildSchema(false, nil)
	if err != nil {
		t.Fatal(err)
	}
	selected, err := buildSchema(true, nil)
	if err != nil {
		t.Fatal(err)
	}
	for _, rule := range []string{"common::capability-name", "surface02::s02-capability-name"} {
		if err := selected.Validate(rule, "softmax.app.state"); err != nil {
			t.Fatal(err)
		}
		for _, invalid := range []string{"softmax.app.state.extra", "other.app.state", "softmax.app.state\n", "Softmax.app.state"} {
			if selected.Validate(rule, invalid) == nil {
				t.Fatalf("%s accepted %q", rule, invalid)
			}
		}
	}
	msg := map[string]any{"type": "read", "body": map[string]any{
		"coglet": testCoglet, "capability": "softmax.app.state", "args": map[string]any{},
		"grant_epoch": uint64(1), "deadline": uint64(100), "correlation_id": ID(),
	}}
	for _, sch := range []*codec.Schema{selected, anyWireSchema()} {
		if err := sch.Validate("surface::surface-message", msg); err != nil {
			t.Fatal(err)
		}
	}
	if strict.Validate("surface::surface-message", msg) == nil {
		t.Fatal("frozen message accepted app name")
	}
	if selected.Validate("capability-name", "softmax.app.state") == nil {
		t.Fatal("unqualified frozen rule widened")
	}
	if strict.Validate("common::capability-name", "softmax.app.state") == nil {
		t.Fatal("frozen rule widened")
	}
	for _, rule := range []string{"cap-entry", "capabilities", "grant", "grants", "read", "act", "schema-refs", "surface-message"} {
		// Missing generated roots would make buildSchema fail closed above.
		if err := selected.Validate("surface02::s02-"+rule, nil); err == nil {
			t.Fatalf("%s accepted null", rule)
		}
	}
}
