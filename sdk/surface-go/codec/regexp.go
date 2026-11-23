package codec

import (
	"fmt"
	"regexp"
	"strconv"
	"strings"
	"unicode/utf8"
)

type patternMatcher interface{ MatchString(string) bool }

// RE2 rejects repeat counts above 1000. These two portable forms are used by
// frozen surface/speech and hub specs; match their exact languages directly,
// not an approximation or a larger unbounded pattern.
type countedPattern struct {
	min, max uint64
	hex      bool
}

func (p countedPattern) MatchString(s string) bool {
	n := uint64(utf8.RuneCountInString(s))
	if p.hex {
		if len(s)%2 != 0 {
			return false
		}
		for _, c := range s {
			if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f') {
				return false
			}
		}
		n = uint64(len(s) / 2)
	}
	return n >= p.min && n <= p.max
}
func compilePattern(pattern string) (patternMatcher, error) {
	for _, prefix := range []string{`^[\s\S]{`, `^([0-9a-f]{2}){`} {
		if strings.HasPrefix(pattern, prefix) && strings.HasSuffix(pattern, `}$`) {
			parts := strings.Split(strings.TrimSuffix(strings.TrimPrefix(pattern, prefix), `}$`), ",")
			if len(parts) == 2 {
				lo, a := strconv.ParseUint(parts[0], 10, 64)
				hi, b := strconv.ParseUint(parts[1], 10, 64)
				if a != nil || b != nil || lo > hi {
					return nil, fmt.Errorf("invalid regexp bounds: %s", pattern)
				}
				return countedPattern{lo, hi, prefix == `^([0-9a-f]{2}){`}, nil
			}
		}
	}
	return regexp.Compile(`\A(?:` + pattern + `)\z`)
}
