package types_test

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"testing"

	"github.com/softmax-universe/coglet/sdk/surface-go/codec"
	"github.com/softmax-universe/coglet/sdk/surface-go/types"
)

const fixtureRoot = "../../../spec/fixtures"

type fixture struct {
	Name  string          `json:"name"`
	Type  string          `json:"type"`
	Valid *bool           `json:"valid"`
	Value json.RawMessage `json:"value"`
	Hex   string          `json:"cbor_hex"`
}

// jsonValue converts JSON into the generic value model without losing integer
// precision; non-integer numbers become float64 (rejected by the profile).
func jsonValue(t *testing.T, raw json.RawMessage) any {
	t.Helper()
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	var v any
	if err := dec.Decode(&v); err != nil {
		t.Fatal(err)
	}
	return convertJSON(v)
}

func convertJSON(v any) any {
	switch x := v.(type) {
	case json.Number:
		if i, err := strconv.ParseUint(x.String(), 10, 64); err == nil {
			return i
		}
		if i, err := strconv.ParseInt(x.String(), 10, 64); err == nil {
			return i
		}
		return float64(1.5) // any non-integer: outside the profile
	case []any:
		for i := range x {
			x[i] = convertJSON(x[i])
		}
		return x
	case map[string]any:
		for k := range x {
			x[k] = convertJSON(x[k])
		}
		return x
	}
	return v
}

func TestFixtures(t *testing.T) {
	var files []string
	registryBytes, err := os.ReadFile("../../../codegen/contracts.json")
	if err != nil {
		t.Fatal(err)
	}
	var registry map[string]string
	if err := json.Unmarshal(registryBytes, &registry); err != nil {
		t.Fatal(err)
	}
	err = filepath.WalkDir(fixtureRoot, func(p string, d fs.DirEntry, err error) error {
		if err == nil && !d.IsDir() && strings.HasSuffix(p, ".json") {
			rel, err := filepath.Rel(fixtureRoot, p)
			if err != nil {
				return err
			}
			family := strings.Split(filepath.ToSlash(rel), "/")[0]
			switch registry[family] {
			case "draft-validated":
				return nil // Actual CDDL/JSON validation remains in spec/tests.
			case "generated":
			default:
				t.Fatalf("unclassified fixture family %q", family)
			}
			files = append(files, p)
		}
		return err
	})
	if err != nil {
		t.Fatal(err)
	}
	sort.Strings(files)
	var valid, invalid, wire int
	for _, path := range files {
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		var cases []fixture
		if err := json.Unmarshal(data, &cases); err != nil {
			t.Fatalf("%s: nonconforming fixture file: %v", path, err)
		}
		for _, c := range cases {
			c := c
			t.Run(filepath.Base(path)+"/"+c.Name, func(t *testing.T) {
				raw, err := hex.DecodeString(c.Hex)
				switch {
				case c.Valid == nil:
					if err != nil || c.Hex == "" {
						t.Fatalf("bad wire fixture %q", c.Name)
					}
					if _, err := codec.DecodeProfile(raw); !errors.Is(err, codec.ErrProfile) {
						t.Fatalf("expected profile rejection, got %v", err)
					}
					wire++
				case *c.Valid:
					if err != nil {
						t.Fatal(err)
					}
					got, err := types.Decode(c.Type, raw)
					if err != nil {
						t.Fatal(err)
					}
					again, err := types.Encode(c.Type, got)
					if err != nil || !bytes.Equal(again, raw) {
						t.Fatalf("decode/re-encode mismatch: %x %v", again, err)
					}
					direct, err := types.Encode(c.Type, jsonValue(t, c.Value))
					if err != nil || !bytes.Equal(direct, raw) {
						t.Fatalf("encode from JSON mismatch: %x %v", direct, err)
					}
					valid++
				default:
					value := jsonValue(t, c.Value)
					if _, err := types.Encode(c.Type, value); err == nil {
						t.Fatal("encode accepted a semantically invalid value")
					}
					// Decode side: profile-encode the value (when possible) and
					// require shape rejection.
					if raw, err := codec.EncodeProfile(value); err == nil {
						if _, err := types.Decode(c.Type, raw); err == nil {
							t.Fatal("decode accepted a semantically invalid item")
						}
					}
					invalid++
				}
			})
		}
	}
	t.Logf("Go fixtures: %d golden, %d semantic-invalid, %d wire-invalid", valid, invalid, wire)
	if valid < 33 || invalid < 43 || wire < 17 {
		t.Fatalf("fixture corpus counts shrank: %d/%d/%d (want at least 33/43/17)", valid, invalid, wire)
	}
}
