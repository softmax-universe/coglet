package codec

import "testing"

func TestStructuredLiteralExactProfileTypes(t *testing.T) {
	s := MustParseSchema(`{"v":{"op":"literal","value":{"items":[true,18446744073709551615,-9223372036854775808]}}}`)
	good := map[string]any{"items": []any{true, uint64(18446744073709551615), int64(-9223372036854775808)}}
	if err := s.Validate("v", good); err != nil {
		t.Fatal(err)
	}
	bad := map[string]any{"items": []any{uint64(1), uint64(18446744073709551615), int64(-9223372036854775808)}}
	if err := s.Validate("v", bad); err == nil {
		t.Fatal("nested bool/int literal confusion")
	}
	for _, value := range []string{`18446744073709551616`, `-9223372036854775809`, `1.5`} {
		if _, err := ParseSchema(`{"v":{"op":"literal","value":{"nested":` + value + `}}}`); err == nil {
			t.Fatalf("invalid literal %s admitted", value)
		}
	}
}
