package main

import (
	"testing"

	surface "github.com/softmax-universe/coglet/sdk/surface-go"
	"github.com/softmax-universe/coglet/sdk/surface-go/types"
)

func TestFixtureGrantsFilterOnlyNegotiatedDowngrade(t *testing.T) {
	entries := []types.Grant{{Operations: []string{"softmax.app.open", "expression.set_expression"}}}
	for _, version := range []string{"", "0.2"} {
		got := fixtureGrants(surface.ProfileSurface02, version, entries)
		if len(got) != 1 || len(got[0].Operations) != 2 {
			t.Fatalf("version %q hid invalid or unnegotiated grants: %v", version, got)
		}
	}
	got := fixtureGrants(surface.ProfileSurface02, "0.1", entries)
	if len(got) != 1 || len(got[0].Operations) != 1 || got[0].Operations[0] != "expression.set_expression" {
		t.Fatalf("downgrade: %v", got)
	}
	if len(entries[0].Operations) != 2 {
		t.Fatal("downgrade mutated provisioning")
	}
	got = fixtureGrants(surface.ProfileSurface01, "0.1", entries)
	if len(got[0].Operations) != 2 {
		t.Fatal("default provisioning silently repaired invalid grants")
	}
}
