package surface

import (
	"encoding/base64"
	"github.com/softmax-universe/coglet/sdk/surface-go/types"
	"strings"
	"testing"
)

func TestCatalogAuditClosedArtifact(t *testing.T) {
	cases := map[string]string{
		"three_component_entry":    strings.ReplaceAll(appCatalog, `softmax.app_sensitive`, `softmax.app.sensitive`),
		"three_component_resource": strings.ReplaceAll(appCatalog, `softmax.app_ctl`, `softmax.app.ctl`),
		"surrogate":                strings.Replace(appCatalog, `"empty": {"op":"record","fields":[]}`, `"empty": {"op":"literal","value":"\ud800"}`, 1),
		"case_alias":               strings.Replace(appCatalog, `"namespace"`, `"Namespace"`, 1),
		"oversized_literal":        strings.Replace(appCatalog, `"empty": {"op":"record","fields":[]}`, `"empty": {"op":"literal","value":18446744073709551616}`, 1),
		"unicode_shorthand":        strings.Replace(appCatalog, `"empty": {"op":"record","fields":[]}`, `"empty": {"op":"regexp","pattern":"\\W+"}`, 1),
	}
	for label, raw := range cases {
		t.Run(label, func(t *testing.T) {
			if _, err := NewTrustedCatalog([]byte(raw), pin(raw)); err == nil {
				t.Fatal("malformed or nonportable catalog admitted")
			}
		})
	}
}

func TestCatalogAuditRetainsExactBytes(t *testing.T) {
	raw := appCatalog + "\n"
	c := mustCatalog(t, raw)
	h := newPHost(t)
	s := h.open(t, ProfileSurface02, c, true)
	// The ledger retains verified distribution bytes, not a reserialized artifact.
	if m := ledgerMap(t, h.cfg(ProfileSurface02, c, false).Ledger); m["Catalog"] != base64.StdEncoding.EncodeToString([]byte(raw)) {
		t.Fatal("ledger lost exact catalog bytes")
	}
	_ = s.Close()
}

func TestCatalogAuditNegotiationWrongKind(t *testing.T) {
	h := newPHost(t)
	s := h.open(t, ProfileSurface02, nil, true)
	raw, err := s.Open(testID)
	must(t, err)
	if err := s.Receive(raw); err == nil {
		t.Fatal("non-reply accepted during negotiation")
	}
}

func TestCatalogAuditScalarEscapesAndPortableRules(t *testing.T) {
	for _, value := range []string{`"\ud83d\ude00"`, `"\\ud800"`, `{"items":[true,1,null]}`} {
		raw := strings.Replace(appCatalog, `"empty": {"op":"record","fields":[]}`, `"empty": {"op":"literal","value":`+value+`}`, 1)
		mustCatalog(t, raw)
	}
	for _, pattern := range []string{`^[a-z]+$`, `^[\s\S]{0,16384}$`, `^(?:yes|no)$`} {
		if err := portablePattern(pattern); err != nil {
			t.Fatalf("portable pattern %q: %v", pattern, err)
		}
	}
	for _, pattern := range []string{`\W+`, `\D+`, `\S+`, `(?i)k`, `[[:alpha:]]+`, `(?=a)a`, `(a)\1`, `[a&&b]`, `a++`} {
		if err := portablePattern(pattern); err == nil {
			t.Fatalf("nonportable pattern %q admitted", pattern)
		}
	}
}

func TestCatalogAuditResourceGrammarIsNotOperationGrammar(t *testing.T) {
	h := newPHost(t)
	s := h.open(t, ProfileSurface02, nil, true)
	op, err := s.BoundOperation("expression.set_expression", nil, func(map[string]any, RequestContext) (map[string]any, error) { return map[string]any{}, nil })
	must(t, err)
	op.Resources = append(op.Resources, "softmax.app.control")
	if err := s.Register(op); err == nil {
		t.Fatal("three-component registered resource accepted")
	}
	handshake(t, s, "0.2")
	if err := s.Grant([]types.Grant{{Coglet: testCoglet, Operations: []string{}, Resources: []string{"softmax.app.control"}, Expires: 1000000}}); err == nil {
		t.Fatal("three-component grant resource accepted")
	}
}
