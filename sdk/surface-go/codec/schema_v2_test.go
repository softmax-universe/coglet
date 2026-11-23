package codec

import (
	"fmt"
	"strings"
	"testing"
)

func TestLargePortableRegexp(t *testing.T) {
	s := MustParseSchema(`{"text":{"op":"regexp","pattern":"^[\\s\\S]{0,16384}$"},"hex":{"op":"regexp","pattern":"^([0-9a-f]{2}){1,32768}$"}}`)
	assertShape(t, s, "text", strings.Repeat("é\n", 8192), true)
	assertShape(t, s, "text", strings.Repeat("é", 16385), false)
	assertShape(t, s, "hex", strings.Repeat("ff", 32768), true)
	for _, value := range []string{"", "0", "FF", strings.Repeat("ff", 32769)} {
		assertShape(t, s, "hex", value, false)
	}
}

// Exercise both public paths: validation is never encode-only.
func assertShape(t *testing.T, s *Schema, rule string, value any, valid bool) {
	t.Helper()
	_, encErr := s.Encode(rule, value)
	raw, err := EncodeProfile(value)
	if err != nil {
		t.Fatal(err)
	}
	_, decErr := s.Decode(rule, raw)
	if (encErr == nil) != valid || (decErr == nil) != valid {
		t.Fatalf("%s(%#v) valid=%v: encode=%v decode=%v", rule, value, valid, encErr, decErr)
	}
}

func TestV2IntersectionDefault(t *testing.T) {
	s := MustParseSchema(`{"text":{"op":"intersection","items":[{"op":"primitive","name":"tstr"},{"op":"size","min":2,"max":3}]},"empty":{"op":"intersection","items":[{"op":"literal","value":null},{"op":"literal","value":false}]},"default":{"op":"default","item":{"op":"literal","value":true},"value":false}}`)
	assertShape(t, s, "text", "ab", true)
	assertShape(t, s, "text", "a", false)
	assertShape(t, s, "text", true, false)
	assertShape(t, s, "empty", nil, false)
	assertShape(t, s, "empty", false, false)
	assertShape(t, s, "default", true, true)
	assertShape(t, s, "default", false, false)
}

func TestV2DefaultsNeverInsertFields(t *testing.T) {
	s := MustParseSchema(`{"record":{"op":"record","fields":[{"name":"optional","optional":true,"type":{"op":"default","item":{"op":"literal","value":true},"value":true}}]}}`)
	raw, err := s.Encode("record", map[string]any{})
	if err != nil || len(raw) != 1 || raw[0] != 0xa0 {
		t.Fatalf("default changed wire: %x %v", raw, err)
	}
	v, err := s.Decode("record", raw)
	if err != nil || len(v.(map[string]any)) != 0 {
		t.Fatalf("default inserted on decode: %#v %v", v, err)
	}
	assertShape(t, s, "record", map[string]any{"optional": false}, false)
}

func TestV2TupleBacktracking(t *testing.T) {
	s := MustParseSchema(`{"tuple":{"op":"tuple","items":[{"type":{"op":"primitive","name":"tstr"},"min":0},{"type":{"op":"literal","value":"end"},"min":1,"max":1}]},"optional":{"op":"tuple","items":[{"type":{"op":"primitive","name":"tstr"},"min":0,"max":1},{"type":{"op":"primitive","name":"tstr"},"min":1,"max":1}]}}`)
	for _, v := range []any{[]string{"end"}, []string{"a", "end"}, []string{"end", "end"}} {
		assertShape(t, s, "tuple", v, true)
	}
	for _, v := range []any{[]string{}, []string{"a"}, []any{true, "end"}} {
		assertShape(t, s, "tuple", v, false)
	}
	assertShape(t, s, "optional", []string{"one"}, true)
	assertShape(t, s, "optional", []string{"one", "two"}, true)
	assertShape(t, s, "optional", []string{"one", "two", "three"}, false)
}

func TestV2CollectionBoundsAndKeys(t *testing.T) {
	s := MustParseSchema(`{"array":{"op":"array","item":{"op":"primitive","name":"bool"},"min":1,"max":2},"map":{"op":"map","item":{"op":"primitive","name":"bool"},"key":{"op":"regexp","pattern":"x+"},"min":1,"max":2},"file::record":{"op":"record","fields":[{"name":"fixed","optional":false,"type":{"op":"literal","value":true}}],"rest":{"item":{"op":"primitive","name":"bool"},"key":{"op":"regexp","pattern":"x+"},"min":1,"max":2}}}`)
	for i := 0; i < 4; i++ {
		assertShape(t, s, "array", make([]bool, i), i >= 1 && i <= 2)
	}
	assertShape(t, s, "map", map[string]bool{"x": true}, true)
	assertShape(t, s, "map", map[string]bool{}, false)
	assertShape(t, s, "map", map[string]bool{"bad": true}, false)
	assertShape(t, s, "map", map[string]bool{"x": true, "xx": true, "xxx": true}, false)
	assertShape(t, s, "file::record", map[string]bool{"fixed": true, "x": true}, true)
	assertShape(t, s, "file::record", map[string]bool{"fixed": true}, false)
	assertShape(t, s, "file::record", map[string]bool{"fixed": true, "bad": true}, false)
	assertShape(t, s, "file::record", map[string]bool{"fixed": true, "x": true, "xx": true, "xxx": true}, false)
	assertShape(t, s, "file::record", map[string]any{"fixed": true, "x": "wrong"}, false)
}

func TestV2SchemaRejectsBadBoundsAndReferences(t *testing.T) {
	for _, ir := range []string{
		`{"op":"array","item":{"op":"literal","value":true},"min":-1}`,
		`{"op":"map","item":{"op":"literal","value":true},"min":2,"max":1}`,
		`{"op":"tuple","items":[{"type":{"op":"literal","value":true},"min":1,"max":0}]}`,
		`{"op":"map","item":{"op":"literal","value":true},"key":{"op":"ref","name":"missing"}}`,
		`{"op":"record","fields":[],"rest":{"item":{"op":"ref","name":"missing"},"min":0}}`,
	} {
		if _, err := ParseSchema(fmt.Sprintf(`{"bad":%s}`, ir)); err == nil {
			t.Fatalf("accepted %s", ir)
		}
	}
}
