package surface

import (
	"bytes"
	"encoding/json"
	"errors"
	"regexp"
	"strconv"
	"strings"
)

// encoding/json repairs lone surrogate escapes. A pin must authenticate the
// scalar values actually distributed, never replacement-character repairs.
func scalarJSON(raw []byte) error {
	inString := false
	for i := 0; i < len(raw); i++ {
		if raw[i] == '"' {
			inString = !inString
			continue
		}
		if !inString || raw[i] != '\\' {
			continue
		}
		i++
		if i >= len(raw) {
			return errors.New("truncated JSON escape")
		}
		if raw[i] != 'u' {
			continue
		}
		if i+4 >= len(raw) {
			return errors.New("truncated Unicode escape")
		}
		n, err := strconv.ParseUint(string(raw[i+1:i+5]), 16, 16)
		if err != nil {
			return err
		}
		i += 4
		if n >= 0xdc00 && n <= 0xdfff {
			return errors.New("unpaired low surrogate")
		}
		if n < 0xd800 || n > 0xdbff {
			continue
		}
		if i+6 >= len(raw) || raw[i+1] != '\\' || raw[i+2] != 'u' {
			return errors.New("unpaired high surrogate")
		}
		low, err := strconv.ParseUint(string(raw[i+3:i+7]), 16, 16)
		if err != nil || low < 0xdc00 || low > 0xdfff {
			return errors.New("unpaired high surrogate")
		}
		i += 6
	}
	return nil
}

func exactKeys(v any, required, optional []string) error {
	m, ok := v.(map[string]any)
	if !ok {
		return errors.New("expected catalog object")
	}
	allowed := map[string]bool{}
	for _, k := range required {
		if _, ok := m[k]; !ok {
			return errors.New("missing catalog field: " + k)
		}
		allowed[k] = true
	}
	for _, k := range optional {
		allowed[k] = true
	}
	for k := range m {
		if !allowed[k] {
			return errors.New("unknown catalog field: " + k)
		}
	}
	return nil
}

// DisallowUnknownFields alone is insufficient: it accepts case aliases for
// struct fields. Check the closed artifact's exact spellings before decoding.
func exactArtifactJSON(raw []byte) error {
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	var v any
	if err := dec.Decode(&v); err != nil {
		return err
	}
	if err := exactKeys(v, []string{"version", "namespace", "rules", "schemas", "entries", "operations"}, nil); err != nil {
		return err
	}
	ops, ok := v.(map[string]any)["operations"].(map[string]any)
	if !ok {
		return errors.New("invalid operations")
	}
	for _, op := range ops {
		if err := exactKeys(op, []string{"entry", "direction", "input", "resources", "single_owner"}, []string{"output", "selection"}); err != nil {
			return err
		}
		if sel := op.(map[string]any)["selection"]; sel != nil {
			if err := exactKeys(sel, []string{"operation_field", "args_field", "value_field", "reads"}, nil); err != nil {
				return err
			}
		}
	}
	return nil
}

// Until the SDKs use Rust's Unicode regex engine, admit only patterns whose
// language is shared by Rust, Go RE2 and Python re.ASCII. Refusal is safer than
// silently widening e.g. \\W to include Unicode letters. Frozen rules are not
// changed. The universal [\\s\\S] class remains portable as a whole.
func portablePattern(pattern string) error {
	if regexp.MustCompile(`^\^\[\\s\\S\]\{[0-9]+,[0-9]+\}\$$`).MatchString(pattern) {
		return nil
	}
	p := pattern
	if strings.Contains(p, "(?") && strings.Contains(strings.ReplaceAll(p, "(?:", ""), "(?") {
		return errors.New("nonportable regexp group")
	}
	for _, marker := range []string{"[:", "&&", "--", "~~", "||"} {
		if strings.Contains(p, marker) {
			return errors.New("nonportable regexp class")
		}
	}
	inClass := false
	for i := 0; i < len(p); i++ {
		switch p[i] {
		case '\\':
			i++
			if i == len(p) {
				return errors.New("truncated regexp escape")
			}
			c := p[i]
			if (c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9') && !strings.ContainsRune("nrtfx", rune(c)) {
				return errors.New("nonportable regexp escape")
			}
			if c == 'x' {
				if i+2 >= len(p) {
					return errors.New("nonportable hex escape")
				}
				if _, err := strconv.ParseUint(p[i+1:i+3], 16, 8); err != nil {
					return err
				}
				i += 2
			}
		case '[':
			if inClass {
				return errors.New("nested regexp class")
			}
			inClass = true
		case ']':
			inClass = false
		case '+', '*', '?', '}':
			if i+1 < len(p) && p[i+1] == '+' {
				return errors.New("nonportable possessive repetition")
			}
		}
	}
	return nil
}
