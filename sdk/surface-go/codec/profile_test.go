package codec_test

import (
	"encoding/hex"
	"errors"
	"strings"
	"testing"

	"github.com/softmax-universe/coglet/sdk/surface-go/codec"
)

func h(t *testing.T, s string) []byte {
	t.Helper()
	b, err := hex.DecodeString(strings.ReplaceAll(s, " ", ""))
	if err != nil {
		t.Fatal(err)
	}
	return b
}

func TestDecodeRejectsNonProfile(t *testing.T) {
	cases := map[string]string{
		"tag-55799":           "d9d9f700",
		"simple-20-extended":  "f814",
		"simple-21-extended":  "f815",
		"simple-22-extended":  "f816",
		"tag-inside-map":      "a16161d9d9f700",
		"negative-below-2^63": "3b8000000000000000",
		"negative-2^64":       "3bffffffffffffffff",
		"huge-array-length":   "9bffffffffffffffff",
		"huge-map-length":     "bbffffffffffffffff",
		"huge-text-length":    "7bffffffffffffffff",
		"nesting-66":          strings.Repeat("81", 66) + "00",
		"bytes":               "4100",
		"float":               "fb3ff0000000000000",
		"indefinite-text":     "7f6161ff",
		"nonshortest-u16":     "190001",
		"nonshortest-u32":     "1a0000ffff",
		"nonshortest-u64":     "1b00000000ffffffff",
		"reserved-ai-28":      "1c",
		"empty":               "",
		"dup-keys":            "a2616101616102",
		"nontext-key":         "a10101",
		"unsorted-keys":       "a2617a01616102",
	}
	cases["longer-key-first"] = "a2626161" + "01" + "6162" + "02"
	for name, in := range cases {
		raw := h(t, in)
		if _, err := codec.DecodeProfile(raw); !errors.Is(err, codec.ErrProfile) {
			t.Errorf("%s: want ErrProfile, got %v", name, err)
		}
	}
}

func TestDecodeAcceptsBoundaries(t *testing.T) {
	good := map[string]any{
		"1bffffffffffffffff": uint64(1<<64 - 1),
		"3b7fffffffffffffff": int64(-1 << 63),
		"20":                 int64(-1),
		"f6":                 nil,
		"f5":                 true,
		"60":                 "",
	}
	for in, want := range good {
		got, err := codec.DecodeProfile(h(t, in))
		if err != nil || got != want {
			t.Errorf("%s: got %v, %v want %v", in, got, err, want)
		}
	}
	// Nesting depth 64 below the root is allowed; 65 is not.
	if _, err := codec.DecodeProfile(h(t, strings.Repeat("81", 64)+"00")); err != nil {
		t.Errorf("depth 64: %v", err)
	}
	// Multi-byte key ordering is by encoded bytes, not length-first.
	if _, err := codec.DecodeProfile(h(t, "a2"+"62"+"6161"+"01"+"62"+"6162"+"02")); err != nil {
		t.Errorf("sorted keys rejected: %v", err)
	}
}

func TestEncodeProfileBoundsAndOrder(t *testing.T) {
	for _, v := range []any{[]byte("a"), 1.5, float32(1), map[int]any{1: 1}, struct{}{}, "\xff"} {
		if _, err := codec.EncodeProfile(v); !errors.Is(err, codec.ErrProfile) {
			t.Errorf("%#v: want ErrProfile, got %v", v, err)
		}
	}
	if raw, err := codec.EncodeProfile(uint64(1<<64 - 1)); err != nil || hex.EncodeToString(raw) != "1bffffffffffffffff" {
		t.Errorf("uint64 max: %x %v", raw, err)
	}
	if raw, err := codec.EncodeProfile(int64(-1 << 63)); err != nil || hex.EncodeToString(raw) != "3b7fffffffffffffff" {
		t.Errorf("int64 min: %x %v", raw, err)
	}
	// Bytewise encoded-key order: "b" (61 62) < "aa" (62 61 61).
	raw, err := codec.EncodeProfile(map[string]any{"aa": 1, "b": 2, "a": 3})
	if err != nil || hex.EncodeToString(raw) != "a3"+"6161"+"03"+"6162"+"02"+"626161"+"01" {
		t.Errorf("order: %x %v", raw, err)
	}
	deep := any(uint64(0))
	for i := 0; i < 65; i++ {
		deep = []any{deep}
	}
	if _, err := codec.EncodeProfile(deep); !errors.Is(err, codec.ErrProfile) {
		t.Errorf("65 levels: %v", err)
	}
	// Typed slices/maps/pointers go through reflection.
	n := 7
	if raw, err := codec.EncodeProfile(map[string][]*int{"k": {&n, nil}}); err != nil || hex.EncodeToString(raw) != "a1616b8207f6" {
		t.Errorf("typed: %x %v", raw, err)
	}
}

func TestSchemaDepthAndUnknownOps(t *testing.T) {
	if _, err := codec.ParseSchema(`{"a":{"op":"cbor-bytes"}}`); !errors.Is(err, codec.ErrSchema) {
		t.Errorf("unknown op: %v", err)
	}
	if _, err := codec.ParseSchema(`{"a":{"op":"ref","name":"missing"}}`); !errors.Is(err, codec.ErrSchema) {
		t.Errorf("undefined ref: %v", err)
	}
	s := codec.MustParseSchema(`{"v":{"op":"union","items":[{"op":"primitive","name":"tstr"},{"op":"array","min":0,"item":{"op":"ref","name":"v"}}]}}`)
	deep := any("x")
	for i := 0; i < 40; i++ {
		deep = []any{deep}
	}
	if err := s.Validate("v", deep); err != nil {
		t.Errorf("40 levels: %v", err)
	}
	for i := 0; i < 8; i++ { // ref+union+array = 3 schema levels per data level (48 total > 42)
		deep = []any{deep}
	}
	if err := s.Validate("v", deep); !errors.Is(err, codec.ErrShape) {
		t.Errorf("schema depth limit: %v", err)
	}
}
