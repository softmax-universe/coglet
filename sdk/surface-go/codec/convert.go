package codec

import "fmt"

func mismatch(want string, got any) error {
	return fmt.Errorf("%w: expected %s, got %T", ErrShape, want, got)
}

// AsAny returns x unchanged.
func AsAny(x any) (any, error) { return x, nil }

// AsString converts a generic value to a string.
func AsString(x any) (string, error) {
	s, ok := x.(string)
	if !ok {
		return "", mismatch("text", x)
	}
	return s, nil
}

// AsBool converts a generic value to a bool.
func AsBool(x any) (bool, error) {
	b, ok := x.(bool)
	if !ok {
		return false, mismatch("bool", x)
	}
	return b, nil
}

// AsUint64 converts a non-negative integer.
func AsUint64(x any) (uint64, error) {
	switch v := x.(type) {
	case uint64:
		return v, nil
	case int64:
		if v >= 0 {
			return uint64(v), nil
		}
	}
	return 0, mismatch("uint64", x)
}

// AsInt64 converts an integer that fits int64.
func AsInt64(x any) (int64, error) {
	switch v := x.(type) {
	case int64:
		return v, nil
	case uint64:
		if v <= 1<<63-1 {
			return int64(v), nil
		}
	}
	return 0, mismatch("int64", x)
}

// AsList converts a generic array element-wise.
func AsList[T any](x any, f func(any) (T, error)) ([]T, error) {
	list, ok := x.([]any)
	if !ok {
		return nil, mismatch("array", x)
	}
	out := make([]T, 0, len(list))
	for _, e := range list {
		t, err := f(e)
		if err != nil {
			return nil, err
		}
		out = append(out, t)
	}
	return out, nil
}

// AsMap converts a generic text-key map value-wise.
func AsMap[T any](x any, f func(any) (T, error)) (map[string]T, error) {
	m, ok := x.(map[string]any)
	if !ok {
		return nil, mismatch("map", x)
	}
	out := make(map[string]T, len(m))
	for k, e := range m {
		t, err := f(e)
		if err != nil {
			return nil, err
		}
		out[k] = t
	}
	return out, nil
}

// AsObject returns a generic open map unchanged.
func AsObject(x any) (map[string]any, error) {
	m, ok := x.(map[string]any)
	if !ok {
		return nil, mismatch("map", x)
	}
	return m, nil
}
