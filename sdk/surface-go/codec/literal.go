package codec

import (
	"encoding/json"
	"fmt"
)

// JSON schema literals use the same closed value profile as wire CBOR. Convert
// JSON integer tokens without routing them through float64 (or conflating bool).
func literalValue(v any, depth int) (any, error) {
	if depth > MaxDepth {
		return nil, fmt.Errorf("literal nesting limit")
	}
	switch x := v.(type) {
	case nil, bool, string:
		return x, nil
	case json.Number:
		n, err := asBig(x)
		if err != nil {
			return nil, err
		}
		if n.Sign() < 0 && n.IsInt64() {
			return n.Int64(), nil
		}
		if n.Sign() >= 0 && n.IsUint64() {
			return n.Uint64(), nil
		}
		return nil, fmt.Errorf("literal integer outside profile")
	case []any:
		out := make([]any, len(x))
		for i, item := range x {
			v, err := literalValue(item, depth+1)
			if err != nil {
				return nil, err
			}
			out[i] = v
		}
		return out, nil
	case map[string]any:
		out := make(map[string]any, len(x))
		for k, item := range x {
			v, err := literalValue(item, depth+1)
			if err != nil {
				return nil, err
			}
			out[k] = v
		}
		return out, nil
	}
	return nil, fmt.Errorf("unsupported literal %T", v)
}
