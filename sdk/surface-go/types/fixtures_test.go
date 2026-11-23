package types_test

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
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
	Name     string          `json:"name"`
	Type     string          `json:"type"`
	Valid    *bool           `json:"valid"`
	Semantic bool            `json:"semantic"`
	Value    json.RawMessage `json:"value"`
	Hex      string          `json:"cbor_hex"`
}

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
		return float64(1.5) // outside the wire profile
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

// Fixture selection follows contracts.json, never the availability of a type.
func fixtureFiles(root, registry string) ([]string, int, error) {
	data, err := os.ReadFile(registry)
	if err != nil {
		return nil, 0, err
	}
	var contracts map[string]string
	if err = json.Unmarshal(data, &contracts); err != nil {
		return nil, 0, err
	}
	if len(contracts) == 0 {
		return nil, 0, fmt.Errorf("empty contract registry")
	}
	for family, mode := range contracts {
		if mode != "generated" && mode != "draft-validated" {
			return nil, 0, fmt.Errorf("unknown contract mode %q", mode)
		}
		info, err := os.Stat(filepath.Join(root, family))
		if err != nil {
			return nil, 0, err
		}
		if !info.IsDir() {
			return nil, 0, fmt.Errorf("not a fixture family: %s", family)
		}
	}
	var files []string
	draft := 0
	err = filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() || !strings.HasSuffix(path, ".json") {
			return nil
		}
		rel, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		parts := strings.Split(rel, string(filepath.Separator))
		if len(parts) < 2 {
			return fmt.Errorf("unclassified fixture %s", path)
		}
		mode, ok := contracts[parts[0]]
		if !ok {
			return fmt.Errorf("unclassified fixture family %s", parts[0])
		}
		if mode == "draft-validated" {
			draft++
			return nil
		}
		files = append(files, path)
		return nil
	})
	sort.Strings(files)
	return files, draft, err
}

func readCases(path string) ([]fixture, bool, error) {
	marker, err := os.ReadFile(path + ".format")
	if err == nil {
		if string(marker) != "semantic-vectors-v1" && string(marker) != "semantic-vectors-v1\n" {
			return nil, false, fmt.Errorf("unknown fixture format %q", marker)
		}
		return nil, true, nil
	}
	if !os.IsNotExist(err) {
		return nil, false, err
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, false, err
	}
	if len(bytes.TrimSpace(data)) == 0 || bytes.TrimSpace(data)[0] != '[' {
		return nil, false, fmt.Errorf("fixture is not a case array")
	}
	var cases []fixture
	if err = json.Unmarshal(data, &cases); err != nil {
		return nil, false, err
	}
	for _, c := range cases {
		if c.Name == "" {
			return nil, false, fmt.Errorf("unnamed fixture")
		}
		if c.Valid == nil {
			if c.Type != "" || c.Semantic || len(c.Value) != 0 || c.Hex == "" {
				return nil, false, fmt.Errorf("malformed wire fixture %s", c.Name)
			}
		} else if c.Type == "" || len(c.Value) == 0 {
			return nil, false, fmt.Errorf("malformed shape fixture %s", c.Name)
		}
	}
	return cases, false, nil
}

func TestFixtures(t *testing.T) {
	files, draft, err := fixtureFiles(fixtureRoot, "../../../codegen/contracts.json")
	if err != nil {
		t.Fatal(err)
	}
	schema := codec.MustParseSchema(types.SchemaJSON)
	var valid, invalid, wire, semantic, skipped int
	for _, path := range files {
		cases, skip, err := readCases(path)
		if err != nil {
			t.Fatalf("%s: %v", path, err)
		}
		if skip {
			skipped++
			continue
		}
		for _, c := range cases {
			t.Run(path+"/"+c.Name, func(t *testing.T) {
				raw, err := hex.DecodeString(c.Hex)
				if err != nil {
					t.Fatal(err)
				}
				if c.Valid == nil {
					if _, err := codec.DecodeProfile(raw); !errors.Is(err, codec.ErrProfile) {
						t.Fatalf("expected profile rejection, got %v", err)
					}
					wire++
					return
				}
				// Unknown rules must not masquerade as successful shape negatives.
				if !schema.HasRule(c.Type) {
					t.Fatalf("unknown fixture type %q", c.Type)
				}
				value := jsonValue(t, c.Value)
				if *c.Valid || c.Semantic {
					direct, err := types.Encode(c.Type, value)
					if err != nil {
						t.Fatal(err)
					}
					if c.Hex != "" && !bytes.Equal(direct, raw) {
						t.Fatalf("golden mismatch: %x != %x", direct, raw)
					}
					got, err := types.Decode(c.Type, direct)
					if err != nil {
						t.Fatal(err)
					}
					again, err := types.Encode(c.Type, got)
					if err != nil || !bytes.Equal(again, direct) {
						t.Fatalf("round trip mismatch: %x %v", again, err)
					}
					if c.Semantic {
						semantic++
					} else {
						valid++
					}
				} else {
					if _, err := types.Encode(c.Type, value); err == nil {
						t.Fatal("encode accepted invalid shape")
					}
					if candidate, err := codec.EncodeProfile(value); err == nil {
						if _, err := types.Decode(c.Type, candidate); err == nil {
							t.Fatal("decode accepted invalid shape")
						}
					}
					if c.Hex != "" {
						if _, err := types.Decode(c.Type, raw); err == nil {
							t.Fatal("decode accepted shape-negative golden")
						}
					}
					invalid++
				}
			})
		}
	}
	t.Logf("Go fixtures: %d valid shapes, %d shape-invalid, %d wire-invalid, %d protocol-semantic; %d semantic-vectors-v1 files skipped; %d draft-validated files excluded", valid, invalid, wire, semantic, skipped, draft)
	if valid < 33 || invalid < 43 || wire < 17 {
		t.Fatalf("fixture corpus counts shrank: %d/%d/%d (want at least 33/43/17)", valid, invalid, wire)
	}
}

func TestFixtureDiscoveryMetadata(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, "cases.json")
	write := func(path, text string) {
		t.Helper()
		if err := os.WriteFile(path, []byte(text), 0600); err != nil {
			t.Fatal(err)
		}
	}
	write(path, `{"vectors":[]}`)
	if _, _, err := readCases(path); err == nil {
		t.Fatal("unmarked vectors accepted")
	}
	write(path+".format", "semantic-vectors-v1\n")
	if _, skip, err := readCases(path); err != nil || !skip {
		t.Fatalf("marker: %v %v", skip, err)
	}
	write(path+".format", "semantic-vectors-v2")
	if _, _, err := readCases(path); err == nil {
		t.Fatal("unknown marker accepted")
	}
	if err := os.Remove(path + ".format"); err != nil {
		t.Fatal(err)
	}
	for _, text := range []string{`null`, `[{}]`, `[{"name":"x","valid":false}]`, `[{"name":"x","type":"x","value":1}]`} {
		write(path, text)
		if _, _, err := readCases(path); err == nil {
			t.Fatalf("accepted malformed %s", text)
		}
	}
	write(path, `[{"name":"shape","type":"x","valid":false,"semantic":true,"value":null}]`)
	if _, skip, err := readCases(path); err != nil || skip {
		t.Fatalf("semantic shape: %v", err)
	}
	for _, family := range []string{"live", "draft"} {
		if err := os.Mkdir(filepath.Join(root, family), 0700); err != nil {
			t.Fatal(err)
		}
		write(filepath.Join(root, family, "cases.json"), "[]")
	}
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	registry := filepath.Join(t.TempDir(), "contracts.json")
	write(registry, `{"live":"generated","draft":"draft-validated"}`)
	files, draft, err := fixtureFiles(root, registry)
	if err != nil || len(files) != 1 || draft != 1 {
		t.Fatalf("selection: %v %d %v", files, draft, err)
	}
	write(registry, `{"live":"generated"}`)
	if _, _, err := fixtureFiles(root, registry); err == nil {
		t.Fatal("unclassified family accepted")
	}
}
