package codec

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"regexp"
	"unicode/utf8"
)

// MaxSchemaDepth bounds schema expression traversal during validation.
const MaxSchemaDepth = 128

// ErrShape is wrapped by every schema (CDDL shape) violation.
var ErrShape = errors.New("coglet: value does not match schema")

// ErrSchema is wrapped when the IR itself is unusable.
var ErrSchema = errors.New("coglet: invalid schema IR")

func shapeErr(format string, args ...any) error {
	return fmt.Errorf("%w: %s", ErrShape, fmt.Sprintf(format, args...))
}

type node struct {
	op       string
	name     string // ref, primitive
	lit      any    // literal: nil, bool, string or *big.Int
	min, max *big.Int
	pattern  *regexp.Regexp
	items    []*node // union
	item     *node   // array, map
	fields   []field
	known    map[string]*field
	key      *node
	rest     *node
	entries  []tupleEntry
}

type tupleEntry struct {
	typ      *node
	min, max *big.Int
}

type field struct {
	name     string
	optional bool
	typ      *node
}

// Schema is a parsed validation IR keyed by CDDL rule name.
type Schema struct {
	rules map[string]*node
}

// ParseSchema parses the IR JSON emitted by codegen. Unknown operations fail.
func ParseSchema(irJSON string) (*Schema, error) {
	dec := json.NewDecoder(bytes.NewReader([]byte(irJSON)))
	dec.UseNumber()
	var raw map[string]any
	if err := dec.Decode(&raw); err != nil {
		return nil, fmt.Errorf("%w: %v", ErrSchema, err)
	}
	s := &Schema{rules: make(map[string]*node, len(raw))}
	for name, r := range raw {
		n, err := buildNode(r)
		if err != nil {
			return nil, fmt.Errorf("%w: rule %s: %v", ErrSchema, name, err)
		}
		s.rules[name] = n
	}
	for name, n := range s.rules {
		if err := s.checkRefs(n); err != nil {
			return nil, fmt.Errorf("%w: rule %s: %v", ErrSchema, name, err)
		}
	}
	return s, nil
}

// MustParseSchema is ParseSchema for package initialisation.
func MustParseSchema(irJSON string) *Schema {
	s, err := ParseSchema(irJSON)
	if err != nil {
		panic(err)
	}
	return s
}

func (s *Schema) checkRefs(n *node) error {
	if n.op == "ref" {
		if _, ok := s.rules[n.name]; !ok {
			return fmt.Errorf("undefined rule %s", n.name)
		}
	}
	for _, c := range n.items {
		if err := s.checkRefs(c); err != nil {
			return err
		}
	}
	if n.item != nil {
		if err := s.checkRefs(n.item); err != nil {
			return err
		}
	}
	for _, child := range []*node{n.key, n.rest} {
		if child != nil {
			if err := s.checkRefs(child); err != nil {
				return err
			}
		}
	}
	for _, entry := range n.entries {
		if err := s.checkRefs(entry.typ); err != nil {
			return err
		}
	}
	for _, f := range n.fields {
		if err := s.checkRefs(f.typ); err != nil {
			return err
		}
	}
	return nil
}

func asBig(v any) (*big.Int, error) {
	num, ok := v.(json.Number)
	if !ok {
		return nil, fmt.Errorf("expected integer, got %T", v)
	}
	b, ok := new(big.Int).SetString(num.String(), 10)
	if !ok {
		return nil, fmt.Errorf("bad integer %s", num)
	}
	return b, nil
}

func buildNode(v any) (*node, error) {
	m, ok := v.(map[string]any)
	if !ok {
		return nil, fmt.Errorf("node is %T", v)
	}
	op, _ := m["op"].(string)
	n := &node{op: op}
	var err error
	switch op {
	case "ref":
		if n.name, ok = m["name"].(string); !ok {
			return nil, errors.New("ref without name")
		}
	case "primitive":
		if n.name, ok = m["name"].(string); !ok || (n.name != "tstr" && n.name != "bool" && n.name != "null") {
			return nil, fmt.Errorf("unsupported primitive %v", m["name"])
		}
	case "literal":
		switch lv := m["value"].(type) {
		case nil, bool, string:
			n.lit = lv
		case json.Number:
			if n.lit, err = asBig(lv); err != nil {
				return nil, err
			}
		default:
			return nil, fmt.Errorf("unsupported literal %T", lv)
		}
	case "range", "size":
		if n.min, err = asBig(m["min"]); err != nil {
			return nil, err
		}
		if n.max, err = asBig(m["max"]); err != nil {
			return nil, err
		}
	case "regexp":
		pat, ok := m["pattern"].(string)
		if !ok {
			return nil, errors.New("regexp without pattern")
		}
		// Anchor explicitly: RE2 has no implicit full-match.
		if n.pattern, err = regexp.Compile(`\A(?:` + pat + `)\z`); err != nil {
			return nil, err
		}
	case "union", "intersection":
		list, _ := m["items"].([]any)
		for _, it := range list {
			c, err := buildNode(it)
			if err != nil {
				return nil, err
			}
			n.items = append(n.items, c)
		}
		if len(n.items) == 0 {
			return nil, errors.New("empty union")
		}
	case "default":
		if n.item, err = buildNode(m["item"]); err != nil {
			return nil, err
		}
	case "tuple":
		list, ok := m["items"].([]any)
		if !ok {
			return nil, errors.New("tuple without items")
		}
		for _, it := range list {
			entry, ok := it.(map[string]any)
			if !ok {
				return nil, errors.New("bad tuple entry")
			}
			c, err := buildNode(entry["type"])
			if err != nil {
				return nil, err
			}
			min, max, err := occurrence(entry)
			if err != nil {
				return nil, err
			}
			n.entries = append(n.entries, tupleEntry{c, min, max})
		}
	case "array", "map":
		if n.min, n.max, err = occurrence(m); err != nil {
			return nil, err
		}
		if n.item, err = buildNode(m["item"]); err != nil {
			return nil, err
		}
		if raw, exists := m["key"]; exists {
			if op != "map" {
				return nil, errors.New("array with key constraint")
			}
			if n.key, err = buildNode(raw); err != nil {
				return nil, err
			}
		}
	case "record":
		list, _ := m["fields"].([]any)
		n.known = map[string]*field{}
		for _, it := range list {
			fm, ok := it.(map[string]any)
			if !ok {
				return nil, errors.New("bad field")
			}
			f := field{}
			if f.name, ok = fm["name"].(string); !ok {
				return nil, errors.New("field without name")
			}
			f.optional, _ = fm["optional"].(bool)
			if f.typ, err = buildNode(fm["type"]); err != nil {
				return nil, err
			}
			if _, dup := n.known[f.name]; dup {
				return nil, fmt.Errorf("duplicate field %s", f.name)
			}
			n.fields = append(n.fields, f)
			n.known[f.name] = &n.fields[len(n.fields)-1]
		}
		if raw, exists := m["rest"]; exists {
			rest, ok := raw.(map[string]any)
			if !ok {
				return nil, errors.New("bad record rest")
			}
			copy := make(map[string]any, len(rest)+1)
			for k, v := range rest {
				copy[k] = v
			}
			copy["op"] = "map"
			if n.rest, err = buildNode(copy); err != nil {
				return nil, err
			}
		}
		// Re-point after slice growth.
		for i := range n.fields {
			n.known[n.fields[i].name] = &n.fields[i]
		}
	default:
		return nil, fmt.Errorf("unsupported op %q", op)
	}
	return n, nil
}

// occurrence parses inclusive non-negative bounds; omitted max is unbounded.
func occurrence(m map[string]any) (*big.Int, *big.Int, error) {
	min := big.NewInt(0)
	var max *big.Int
	var err error
	if raw, ok := m["min"]; ok {
		if min, err = asBig(raw); err != nil {
			return nil, nil, err
		}
	}
	if raw, ok := m["max"]; ok {
		if max, err = asBig(raw); err != nil {
			return nil, nil, err
		}
	}
	if min.Sign() < 0 || (max != nil && (max.Sign() < 0 || min.Cmp(max) > 0)) {
		return nil, nil, errors.New("invalid occurrence bounds")
	}
	return min, max, nil
}

func within(count int, min, max *big.Int) bool {
	n := big.NewInt(int64(count))
	return (min == nil || n.Cmp(min) >= 0) && (max == nil || n.Cmp(max) <= 0)
}

// HasRule reports whether the entry rule exists.
func (s *Schema) HasRule(rule string) bool { _, ok := s.rules[rule]; return ok }

// UnionLen returns the number of top-level branches of rule (1 if not a union).
func (s *Schema) UnionLen(rule string) int {
	n, ok := s.rules[rule]
	if !ok {
		return 0
	}
	if n.op == "union" {
		return len(n.items)
	}
	return 1
}

func intOf(v any) (*big.Int, bool) {
	switch x := v.(type) {
	case uint64:
		return new(big.Int).SetUint64(x), true
	case int64:
		return big.NewInt(x), true
	}
	return nil, false
}

func (s *Schema) check(n *node, v any, depth int) error {
	if depth > MaxSchemaDepth {
		return shapeErr("schema recursion limit")
	}
	switch n.op {
	case "ref":
		return s.check(s.rules[n.name], v, depth+1)
	case "default":
		return s.check(n.item, v, depth+1)
	case "intersection":
		for _, b := range n.items {
			if err := s.check(b, v, depth+1); err != nil {
				return err
			}
		}
		return nil
	case "tuple":
		list, ok := v.([]any)
		if !ok {
			return shapeErr("expected tuple")
		}
		// Memoize (entry, position) rather than greedily consuming occurrences.
		type position struct{ entry, offset int }
		failed := map[position]bool{}
		var match func(int, int) bool
		match = func(entry, offset int) bool {
			if entry == len(n.entries) {
				return offset == len(list)
			}
			state := position{entry, offset}
			if failed[state] {
				return false
			}
			e := n.entries[entry]
			for count := 0; offset+count <= len(list); count++ {
				if e.max != nil && big.NewInt(int64(count)).Cmp(e.max) > 0 {
					break
				}
				if within(count, e.min, e.max) && match(entry+1, offset+count) {
					return true
				}
				if offset+count == len(list) || s.check(e.typ, list[offset+count], depth+1) != nil {
					break
				}
			}
			failed[state] = true
			return false
		}
		if match(0, 0) {
			return nil
		}
		return shapeErr("tuple occurrence mismatch")
	case "union":
		for _, b := range n.items {
			if s.check(b, v, depth+1) == nil {
				return nil
			}
		}
		return shapeErr("no union branch matched")
	case "literal":
		switch lit := n.lit.(type) {
		case nil:
			if v == nil {
				return nil
			}
		case bool:
			if b, ok := v.(bool); ok && b == lit {
				return nil
			}
		case string:
			if str, ok := v.(string); ok && str == lit {
				return nil
			}
		case *big.Int:
			if i, ok := intOf(v); ok && i.Cmp(lit) == 0 {
				return nil
			}
		}
		return shapeErr("literal mismatch")
	case "primitive":
		switch n.name {
		case "tstr":
			if _, ok := v.(string); ok {
				return nil
			}
		case "bool":
			if _, ok := v.(bool); ok {
				return nil
			}
		case "null":
			if v == nil {
				return nil
			}
		}
		return shapeErr("expected %s", n.name)
	case "range":
		if i, ok := intOf(v); ok && n.min.Cmp(i) <= 0 && i.Cmp(n.max) <= 0 {
			return nil
		}
		return shapeErr("integer outside range")
	case "regexp":
		if str, ok := v.(string); ok && n.pattern.MatchString(str) {
			return nil
		}
		return shapeErr("text does not match pattern")
	case "size":
		if str, ok := v.(string); ok && utf8.ValidString(str) {
			l := new(big.Int).SetInt64(int64(len(str)))
			if n.min.Cmp(l) <= 0 && l.Cmp(n.max) <= 0 {
				return nil
			}
		}
		return shapeErr("text size outside bounds")
	case "array":
		list, ok := v.([]any)
		if !ok || !within(len(list), n.min, n.max) {
			return shapeErr("array mismatch")
		}
		for _, e := range list {
			if err := s.check(n.item, e, depth+1); err != nil {
				return err
			}
		}
		return nil
	case "map":
		m, ok := v.(map[string]any)
		if !ok {
			return shapeErr("expected map")
		}
		if !within(len(m), n.min, n.max) {
			return shapeErr("map size outside bounds")
		}
		for k, e := range m {
			if n.key != nil {
				if err := s.check(n.key, k, depth+1); err != nil {
					return err
				}
			}
			if err := s.check(n.item, e, depth+1); err != nil {
				return err
			}
		}
		return nil
	case "record":
		m, ok := v.(map[string]any)
		if !ok {
			return shapeErr("expected record")
		}
		rest := map[string]any{}
		for k, e := range m {
			if _, known := n.known[k]; !known {
				if n.rest == nil {
					return shapeErr("unknown field %q", k)
				}
				rest[k] = e
			}
		}
		if n.rest != nil {
			if err := s.check(n.rest, rest, depth+1); err != nil {
				return err
			}
		}
		for i := range n.fields {
			f := &n.fields[i]
			e, present := m[f.name]
			if !present {
				if !f.optional {
					return shapeErr("missing field %q", f.name)
				}
				continue
			}
			if err := s.check(f.typ, e, depth+1); err != nil {
				return err
			}
		}
		return nil
	}
	return shapeErr("unsupported op %s", n.op)
}

// Validate checks an already-generic profile value against an entry rule.
// Non-profile values (floats, bytes, bad bounds) are rejected first.
func (s *Schema) Validate(rule string, v any) error {
	_, err := s.Encode(rule, v)
	return err
}

// ValidateVariant checks v against branch idx of a top-level union rule
// (idx 0 for a non-union rule).
func (s *Schema) ValidateVariant(rule string, idx int, v any) error {
	n, ok := s.rules[rule]
	if !ok {
		return fmt.Errorf("%w: unknown rule %s", ErrShape, rule)
	}
	// Same traversal depth as validating the union as a whole.
	if n.op == "union" {
		if idx < 0 || idx >= len(n.items) {
			return fmt.Errorf("%w: variant %d out of range", ErrShape, idx)
		}
		return s.check(n.items[idx], v, 1)
	}
	if idx != 0 {
		return fmt.Errorf("%w: variant %d out of range", ErrShape, idx)
	}
	return s.check(n, v, 0)
}

// Encode validates v against rule and returns its deterministic CBOR.
func (s *Schema) Encode(rule string, v any) ([]byte, error) {
	n, ok := s.rules[rule]
	if !ok {
		return nil, fmt.Errorf("%w: unknown rule %s", ErrShape, rule)
	}
	raw, err := EncodeProfile(v)
	if err != nil {
		return nil, err
	}
	// Validate the generic form actually encoded, so typed inputs are covered.
	generic, err := DecodeProfile(raw)
	if err != nil {
		return nil, err
	}
	if err := s.check(n, generic, 0); err != nil {
		return nil, err
	}
	return raw, nil
}

// Decode strictly decodes raw and validates it against rule.
func (s *Schema) Decode(rule string, raw []byte) (any, error) {
	n, ok := s.rules[rule]
	if !ok {
		return nil, fmt.Errorf("%w: unknown rule %s", ErrShape, rule)
	}
	v, err := DecodeProfile(raw)
	if err != nil {
		return nil, err
	}
	if err := s.check(n, v, 0); err != nil {
		return nil, err
	}
	return v, nil
}

// EncodeVariant is Encode restricted to branch idx of a top-level union rule.
func (s *Schema) EncodeVariant(rule string, idx int, v any) ([]byte, error) {
	raw, err := EncodeProfile(v)
	if err != nil {
		return nil, err
	}
	generic, err := DecodeProfile(raw)
	if err != nil {
		return nil, err
	}
	if err := s.ValidateVariant(rule, idx, generic); err != nil {
		return nil, err
	}
	return raw, nil
}
