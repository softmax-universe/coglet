package surface

// Local contract profile and trusted catalog provisioning (ADR-0030 section 1a).
//
// The profile is trusted local provisioning, never inferred from a peer, a
// manifest, or the negotiated wire minor. It selects name validation only:
// surface-0.1 keeps the frozen two-component capability grammar, surface-0.2
// adds exactly softmax.app.<component>.

import (
	"bytes"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math/big"
	"regexp"
	"strings"
	"sync"
	"unicode"
	"unicode/utf8"

	"github.com/softmax-universe/coglet/sdk/surface-go/codec"
	"github.com/softmax-universe/coglet/sdk/surface-go/types"
)

// Exact contract profile spellings. Config.ContractProfile == "" is only the Go
// spelling of omission and normalizes to ProfileSurface01.
const (
	ProfileSurface01 = "surface-0.1"
	ProfileSurface02 = "surface-0.2"
)

const (
	wire01        = "0.1"
	wire02        = "0.2"
	maxCatalog    = 1 << 20
	extPrefix     = "ext::"
	appNamePatten = `^softmax\.app\.[a-z][a-z0-9_]{0,31}$`
)

var (
	nameV1 = regexp.MustCompile(`^[a-z][a-z0-9_]{0,31}\.[a-z][a-z0-9_]{0,31}$`)
	nameV2 = regexp.MustCompile(appNamePatten)
	hex64  = regexp.MustCompile(`^[0-9a-f]{64}$`)
)

// NormalizeProfile maps omission to surface-0.1 and refuses every other value
// except the two exact profile strings. There is no fallback for unknowns.
func NormalizeProfile(p string) (string, error) {
	switch p {
	case "", ProfileSurface01:
		return ProfileSurface01, nil
	case ProfileSurface02:
		return ProfileSurface02, nil
	}
	return "", fmt.Errorf("unsupported contract profile %q", p)
}

// nameOK validates a capability name under a profile. Strings are matched whole;
// Go's $ without (?m) cannot match before a trailing newline.
func nameOK(profile, name string) bool {
	if nameV1.MatchString(name) {
		return true
	}
	return profile == ProfileSurface02 && nameV2.MatchString(name)
}

// TrustedCatalog is a verified immutable local extension catalog (closed
// artifact v1, no contract_profile member). Only NewTrustedCatalog creates a
// usable value; the digest covers the exact distribution bytes.
type TrustedCatalog struct {
	raw     []byte // immutable copy of verified distribution bytes
	digest  string
	art     *artifact
	rules   map[string]any // rewritten into the ext:: rule namespace
	in, out map[string]string
}

// Digest returns the verified lowercase SHA-256 of the exact catalog bytes.
func (c *TrustedCatalog) Digest() string {
	if c == nil {
		return ""
	}
	return c.digest
}

type artifact struct {
	Version    json.Number            `json:"version"`
	Namespace  string                 `json:"namespace"`
	Rules      map[string]any         `json:"rules"`
	Schemas    map[string]string      `json:"schemas"`
	Entries    map[string][]string    `json:"entries"`
	Operations map[string]*artifactOp `json:"operations"`
}
type artifactOp struct {
	Entry       string       `json:"entry"`
	Direction   string       `json:"direction"`
	Input       string       `json:"input"`
	Output      *string      `json:"output"`
	Resources   []string     `json:"resources"`
	SingleOwner bool         `json:"single_owner"`
	Selection   *artifactSel `json:"selection"`
}
type artifactSel struct {
	OperationField string   `json:"operation_field"`
	ArgsField      string   `json:"args_field"`
	ValueField     string   `json:"value_field"`
	Reads          []string `json:"reads"`
}

// NewTrustedCatalog verifies the separately pinned digest over the exact bytes,
// then loads the closed v1 artifact. Names are checked against the widest (0.2)
// grammar here; New re-checks them under the configured profile.
func NewTrustedCatalog(raw []byte, sha256Hex string) (*TrustedCatalog, error) {
	sum := sha256.Sum256(raw)
	got := hex.EncodeToString(sum[:])
	if !hex64.MatchString(sha256Hex) || subtle.ConstantTimeCompare([]byte(got), []byte(sha256Hex)) != 1 {
		return nil, errors.New("trusted catalog pin mismatch or absent")
	}
	if len(raw) > maxCatalog {
		return nil, errors.New("trusted catalog exceeds byte limit")
	}
	if err := strictJSON(raw); err != nil {
		return nil, err
	}
	if err := exactArtifactJSON(raw); err != nil {
		return nil, err
	}
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	dec.DisallowUnknownFields()
	var a artifact
	if err := dec.Decode(&a); err != nil {
		return nil, fmt.Errorf("trusted catalog: %w", err)
	}
	if err := a.validate(); err != nil {
		return nil, err
	}
	c := &TrustedCatalog{raw: append([]byte(nil), raw...), digest: got, art: &a, in: map[string]string{}, out: map[string]string{}}
	c.rules = map[string]any{}
	for name, r := range a.Rules {
		c.rules[extPrefix+name] = rewriteRefs(r)
	}
	for name, op := range a.Operations {
		c.in[name] = extPrefix + a.Schemas[op.Input]
		if op.Output != nil {
			c.out[name] = extPrefix + a.Schemas[*op.Output]
		}
	}
	// Whole-IR compile check with the shared validator semantics.
	if _, err := buildSchema(false, c); err != nil {
		return nil, fmt.Errorf("trusted catalog IR: %w", err)
	}
	return c, nil
}

// checkProfile re-validates every name under the configured profile.
func (c *TrustedCatalog) checkProfile(profile string) error {
	ok := func(n string) bool { return nameOK(profile, n) }
	for e, rs := range c.art.Entries {
		if !ok(e) {
			return errors.New("catalog name not valid under contract profile")
		}
		for _, r := range rs {
			if !ok(r) {
				return errors.New("catalog name not valid under contract profile")
			}
		}
	}
	for n, op := range c.art.Operations {
		if !ok(n) {
			return errors.New("catalog name not valid under contract profile")
		}
		for _, r := range op.Resources {
			if !ok(r) {
				return errors.New("catalog name not valid under contract profile")
			}
		}
	}
	return nil
}

var reservedNamespaces = []string{"runtime", "speech", "expression", "llm", "action", "hub", "test", "sim", "sensor", "motor", "other"}

func (a *artifact) validate() error {
	if a.Version.String() != "1" || a.Namespace == "" || strings.Contains(a.Namespace, ".") ||
		len(a.Operations) == 0 || len(a.Rules) == 0 {
		return errors.New("unsupported/reserved extension namespace/version")
	}
	for _, r := range reservedNamespaces {
		if a.Namespace == r {
			return errors.New("unsupported/reserved extension namespace/version")
		}
	}
	prefix := a.Namespace + "."
	name := func(s string) error {
		if !nameOK(ProfileSurface02, s) || !strings.HasPrefix(s, prefix) {
			return errors.New("extension name invalid or outside namespace")
		}
		return nil
	}
	if a.Entries == nil || a.Schemas == nil {
		return errors.New("missing catalog members")
	}
	for entry, resources := range a.Entries {
		if err := name(entry); err != nil || !nameV1.MatchString(entry) || resources == nil {
			return errors.New("invalid catalog entry")
		}
		seen := map[string]bool{}
		for _, r := range resources {
			if err := name(r); err != nil || !nameV1.MatchString(r) || seen[r] {
				return errors.New("invalid or duplicate entry resource")
			}
			seen[r] = true
		}
	}
	for uri, rule := range a.Schemas {
		if !strings.HasPrefix(uri, "https://") || strings.Contains(uri, "coglet.dev/spec/messages/") ||
			strings.IndexFunc(uri, unicode.IsSpace) >= 0 {
			return errors.New("unknown/reserved schema URI")
		}
		if _, ok := a.Rules[rule]; !ok {
			return errors.New("unknown schema rule")
		}
	}
	for rule, e := range a.Rules {
		if err := closedIR(a.Rules, e, map[string]bool{rule: true}, 0); err != nil {
			return err
		}
	}
	for opName, op := range a.Operations {
		if op == nil {
			return errors.New("invalid extension operation binding")
		}
		if err := name(opName); err != nil {
			return err
		}
		if _, builtin := catalog[opName]; builtin || !op.SingleOwner || a.Entries[op.Entry] == nil ||
			(op.Direction != "event" && op.Direction != "read" && op.Direction != "action") {
			return errors.New("invalid extension operation binding")
		}
		if _, ok := a.Schemas[op.Input]; !ok {
			return errors.New("invalid extension operation binding")
		}
		if op.Direction == "event" && op.Output != nil {
			return errors.New("invalid extension operation binding")
		}
		if op.Direction != "event" {
			if op.Output == nil {
				return errors.New("invalid extension operation binding")
			}
			if _, ok := a.Schemas[*op.Output]; !ok {
				return errors.New("invalid extension operation binding")
			}
		}
		if op.Resources == nil {
			return errors.New("invalid extension operation binding")
		}
		seen := map[string]bool{}
		for _, r := range op.Resources {
			ok := false
			for _, d := range a.Entries[op.Entry] {
				ok = ok || d == r
			}
			if !ok || seen[r] {
				return errors.New("unknown/duplicate operation resource")
			}
			seen[r] = true
		}
		if s := op.Selection; s != nil {
			if op.Direction != "event" || len(s.Reads) == 0 || s.OperationField == "" || s.ArgsField == "" || s.ValueField == "" ||
				s.OperationField == s.ArgsField || s.OperationField == s.ValueField || s.ArgsField == s.ValueField {
				return errors.New("invalid selected-read binding")
			}
			seenRead := map[string]bool{}
			for _, r := range s.Reads {
				t := a.Operations[r]
				if seenRead[r] || t == nil || t.Direction != "read" {
					return errors.New("unknown selected read")
				}
				seenRead[r] = true
			}
		}
	}
	if o := a.Operations["game.observation"]; o != nil && o.Selection == nil {
		return errors.New("game.observation requires selected read binding")
	}
	return nil
}

func asInt(v any) (*big.Int, bool) {
	n, ok := v.(json.Number)
	if !ok {
		return nil, false
	}
	i, ok := new(big.Int).SetString(n.String(), 10)
	return i, ok
}

var (
	minBound = new(big.Int).Neg(new(big.Int).Lsh(big.NewInt(1), 63))
	maxBound = new(big.Int).Sub(new(big.Int).Lsh(big.NewInt(1), 64), big.NewInt(1))
)

func bound(v any) (*big.Int, bool) {
	i, ok := asInt(v)
	if !ok || i.Cmp(minBound) < 0 || i.Cmp(maxBound) > 0 {
		return nil, false
	}
	return i, true
}

// closedIR accepts only the finite generated-IR subset with exact key sets.
func closedIR(rules map[string]any, e any, stack map[string]bool, depth int) error {
	if depth > 64 {
		return errors.New("extension schema nesting")
	}
	o, ok := e.(map[string]any)
	if !ok {
		return errors.New("invalid IR expression")
	}
	op, _ := o["op"].(string)
	var keys []string
	switch op {
	case "ref":
		r, ok := o["name"].(string)
		if !ok {
			return errors.New("missing IR reference")
		}
		if stack[r] {
			return errors.New("recursive extension schema")
		}
		target, ok := rules[r]
		if !ok {
			return errors.New("unknown IR reference")
		}
		stack[r] = true
		if err := closedIR(rules, target, stack, depth+1); err != nil {
			return err
		}
		delete(stack, r)
		keys = []string{"op", "name"}
	case "record":
		fs, ok := o["fields"].([]any)
		if !ok {
			return errors.New("missing IR fields")
		}
		seen := map[string]bool{}
		for _, f := range fs {
			fm, ok := f.(map[string]any)
			n, nok := fm["name"].(string)
			_, bok := fm["optional"].(bool)
			if !ok || len(fm) != 3 || !nok || !bok || seen[n] {
				return errors.New("invalid/duplicate IR field")
			}
			seen[n] = true
			if err := closedIR(rules, fm["type"], stack, depth+1); err != nil {
				return err
			}
		}
		keys = []string{"op", "fields"}
	case "union", "intersection":
		items, ok := o["items"].([]any)
		if !ok || len(items) == 0 {
			return errors.New("empty IR alternatives")
		}
		for _, it := range items {
			if err := closedIR(rules, it, stack, depth+1); err != nil {
				return err
			}
		}
		keys = []string{"op", "items"}
	case "array":
		lo, ok1 := asInt(o["min"])
		hi, ok2 := asInt(o["max"])
		if !ok1 || !ok2 || lo.Sign() < 0 || lo.Cmp(hi) > 0 || hi.Cmp(big.NewInt(262144)) > 0 {
			return errors.New("unbounded IR array")
		}
		if err := closedIR(rules, o["item"], stack, depth+1); err != nil {
			return err
		}
		keys = []string{"op", "min", "max", "item"}
	case "primitive":
		if n, _ := o["name"].(string); n != "tstr" && n != "bool" && n != "null" {
			return errors.New("unknown primitive")
		}
		keys = []string{"op", "name"}
	case "range", "size":
		lo, ok1 := bound(o["min"])
		hi, ok2 := bound(o["max"])
		if !ok1 || !ok2 || lo.Cmp(hi) > 0 || (op == "size" && lo.Sign() < 0) {
			return errors.New("invalid IR bounds")
		}
		keys = []string{"op", "min", "max"}
	case "literal":
		// strictJSON already checked every scalar. buildSchema compiles the
		// complete literal (including closed objects/arrays) into profile bytes.
		keys = []string{"op", "value"}
	case "regexp":
		pattern, ok := o["pattern"].(string)
		if !ok {
			return errors.New("missing regexp")
		}
		if err := portablePattern(pattern); err != nil {
			return err
		}
		keys = []string{"op", "pattern"}
	default:
		return errors.New("unsupported/open extension schema IR")
	}
	if len(o) != len(keys) {
		return errors.New("unknown/missing IR field")
	}
	for _, k := range keys {
		if _, ok := o[k]; !ok {
			return errors.New("unknown/missing IR field")
		}
	}
	return nil
}

// strictJSON refuses BOM, invalid UTF-8, duplicate keys (after unescaping),
// excessive nesting and trailing tokens; encoding/json alone accepts them.
func strictJSON(raw []byte) error {
	if err := scalarJSON(raw); err != nil {
		return err
	}
	if !utf8.Valid(raw) || bytes.HasPrefix(raw, []byte("\xef\xbb\xbf")) {
		return errors.New("catalog must be strict UTF-8 without BOM")
	}
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	if err := walkJSON(dec, 0); err != nil {
		return err
	}
	if _, err := dec.Token(); err != io.EOF {
		return errors.New("trailing catalog tokens")
	}
	return nil
}
func walkJSON(dec *json.Decoder, depth int) error {
	if depth > 256 {
		return errors.New("catalog nesting")
	}
	t, err := dec.Token()
	if err != nil {
		return err
	}
	if n, ok := t.(json.Number); ok {
		if _, ok := bound(n); !ok {
			return errors.New("catalog number outside integer profile")
		}
	}
	d, ok := t.(json.Delim)
	if !ok {
		return nil
	}
	if d == '{' {
		seen := map[string]bool{}
		for dec.More() {
			kt, err := dec.Token()
			if err != nil {
				return err
			}
			k := kt.(string)
			if seen[k] {
				return errors.New("duplicate catalog key")
			}
			seen[k] = true
			if err = walkJSON(dec, depth+1); err != nil {
				return err
			}
		}
	} else {
		for dec.More() {
			if err := walkJSON(dec, depth+1); err != nil {
				return err
			}
		}
	}
	_, err = dec.Token()
	return err
}

// rewriteRefs deep-copies IR, moving local refs into the ext:: namespace so
// they can never alias or override a built-in rule.
func rewriteRefs(v any) any {
	switch x := v.(type) {
	case map[string]any:
		out := make(map[string]any, len(x))
		for k, val := range x {
			out[k] = val
		}
		if x["op"] == "literal" {
			return out
		}
		if x["op"] == "ref" {
			if n, ok := x["name"].(string); ok {
				out["name"] = extPrefix + n
			}
			return out
		}
		for k, val := range x {
			if k != "op" && k != "name" && k != "pattern" && k != "value" {
				out[k] = rewriteRefs(val)
			}
		}
		return out
	case []any:
		out := make([]any, len(x))
		for i, val := range x {
			out[i] = rewriteRefs(val)
		}
		return out
	}
	return v
}

// buildSchema selects generated surface02 roots for negotiated 0.2 and adds
// the catalog's local rules under ext::. Frozen generated rules stay unchanged.
func buildSchema(relaxed bool, c *TrustedCatalog) (*codec.Schema, error) {
	dec := json.NewDecoder(strings.NewReader(types.SchemaJSON))
	dec.UseNumber()
	var base map[string]any
	if err := dec.Decode(&base); err != nil {
		return nil, err
	}
	if relaxed {
		families := map[string][]string{
			"common":  {"capability-name", "event", "action"},
			"surface": {"surface-message", "cap-entry", "capabilities", "grant", "grants", "read", "act", "schema-refs"},
		}
		for family, rules := range families {
			for _, rule := range rules {
				target := "surface02::s02-" + rule
				if _, ok := base[target]; !ok {
					return nil, fmt.Errorf("missing generated rule %s", target)
				}
				base[family+"::"+rule] = map[string]any{"op": "ref", "name": target}
			}
		}
	}
	if c != nil {
		for k, v := range c.rules {
			if _, dup := base[k]; dup {
				return nil, errors.New("catalog rule overrides built-in")
			}
			base[k] = v
		}
	}
	raw, err := json.Marshal(base)
	if err != nil {
		return nil, err
	}
	return codec.ParseSchema(string(raw))
}

var (
	anyOnce   sync.Once
	anySchema *codec.Schema
)

// anyWireSchema is the 0.2-widened superset, used only to classify frames by
// type/kind. Validation of a session's frames always uses its selected version.
func anyWireSchema() *codec.Schema {
	anyOnce.Do(func() {
		s, err := buildSchema(true, nil)
		if err != nil {
			panic(err)
		}
		anySchema = s
	})
	return anySchema
}

func encodeWith(s *codec.Schema, kind string, body any) ([]byte, error) {
	raw, err := s.Encode(wire, map[string]any{"type": kind, "body": body})
	if err == nil && (len(raw) == 0 || len(raw) > MaxFrame) {
		err = errors.New("invalid frame size")
	}
	return raw, err
}

// selected-read check for an event binding: the selected read's input and
// output are both validated, then the caller must hold both grants.
func (s *Surface) selectedRead(name string, payload map[string]any) (string, error) {
	sel := s.sel[name]
	if sel == nil {
		return "", nil
	}
	read, ok := payload[sel.OperationField].(string)
	if !ok {
		return "", Refuse("invalid_argument")
	}
	listed := false
	for _, r := range sel.Reads {
		listed = listed || r == read
	}
	rb, registered := s.operations[read]
	if !listed || !registered || rb.binding.Direction != "read" {
		return "", Refuse("invalid_argument")
	}
	if s.wireSchema().Validate(rb.binding.inRule(), payload[sel.ArgsField]) != nil ||
		s.wireSchema().Validate(rb.binding.outRule(), payload[sel.ValueField]) != nil {
		return "", Refuse("invalid_argument")
	}
	return read, nil
}
