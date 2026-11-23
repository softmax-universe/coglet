// Package codec implements the Coglet strict deterministic CBOR profile
// (spec/conventions.md) and a validator driven by the generated CDDL IR.
//
// Generic value model: nil, bool, string, uint64 (non-negative integers),
// int64 (negative integers), []any and map[string]any. Encoders additionally
// accept any Go integer kind, named string/bool kinds, slices, string-keyed
// maps, pointers and values implementing Valuer.
package codec

import (
	"bytes"
	"errors"
	"fmt"
	"math"
	"reflect"
	"sort"
	"unicode/utf8"
)

// MaxDepth bounds CBOR container nesting; the root item has depth 0.
const MaxDepth = 64

// ErrProfile is wrapped by every CBOR profile violation.
var ErrProfile = errors.New("coglet: CBOR profile violation")

func profileErr(format string, args ...any) error {
	return fmt.Errorf("%w: %s", ErrProfile, fmt.Sprintf(format, args...))
}

// Valuer is implemented by generated records so encoders can accept them.
type Valuer interface {
	ToValue() map[string]any
}

func appendHead(dst []byte, major byte, n uint64) []byte {
	m := major << 5
	switch {
	case n < 24:
		return append(dst, m|byte(n))
	case n <= math.MaxUint8:
		return append(dst, m|24, byte(n))
	case n <= math.MaxUint16:
		return append(dst, m|25, byte(n>>8), byte(n))
	case n <= math.MaxUint32:
		return append(dst, m|26, byte(n>>24), byte(n>>16), byte(n>>8), byte(n))
	default:
		return append(dst, m|27, byte(n>>56), byte(n>>48), byte(n>>40), byte(n>>32),
			byte(n>>24), byte(n>>16), byte(n>>8), byte(n))
	}
}

// EncodeProfile encodes v as core deterministic CBOR restricted to the
// profile (no bytes, tags or floats; integers in -2^63..2^64-1).
func EncodeProfile(v any) ([]byte, error) {
	return encodeItem(nil, v, 0)
}

func encodeString(dst []byte, s string) ([]byte, error) {
	if !utf8.ValidString(s) {
		return nil, profileErr("text is not valid UTF-8")
	}
	dst = appendHead(dst, 3, uint64(len(s)))
	return append(dst, s...), nil
}

func encodeItem(dst []byte, v any, depth int) ([]byte, error) {
	if depth > MaxDepth {
		return nil, profileErr("nesting limit")
	}
	switch x := v.(type) {
	case nil:
		return append(dst, 0xf6), nil
	case bool:
		if x {
			return append(dst, 0xf5), nil
		}
		return append(dst, 0xf4), nil
	case string:
		return encodeString(dst, x)
	case uint64:
		return appendHead(dst, 0, x), nil
	case int64:
		return encodeInt64(dst, x), nil
	case int:
		return encodeInt64(dst, int64(x)), nil
	case uint:
		return appendHead(dst, 0, uint64(x)), nil
	case []any:
		dst = appendHead(dst, 4, uint64(len(x)))
		var err error
		for _, e := range x {
			if dst, err = encodeItem(dst, e, depth+1); err != nil {
				return nil, err
			}
		}
		return dst, nil
	case map[string]any:
		return encodeMap(dst, len(x), func(yield func(string, any)) {
			for k, e := range x {
				yield(k, e)
			}
		}, depth)
	case Valuer:
		// A typed nil also implements Valuer, but invoking its value-receiver
		// method would panic. Treat it like other nil pointers (CBOR null).
		rv := reflect.ValueOf(x)
		if rv.Kind() == reflect.Pointer && rv.IsNil() {
			return append(dst, 0xf6), nil
		}
		return encodeItem(dst, x.ToValue(), depth)
	}
	return encodeReflect(dst, reflect.ValueOf(v), depth)
}

func encodeInt64(dst []byte, x int64) []byte {
	if x >= 0 {
		return appendHead(dst, 0, uint64(x))
	}
	return appendHead(dst, 1, uint64(-1-x))
}

func encodeReflect(dst []byte, rv reflect.Value, depth int) ([]byte, error) {
	switch rv.Kind() {
	case reflect.Invalid:
		return append(dst, 0xf6), nil
	case reflect.Bool:
		return encodeItem(dst, rv.Bool(), depth)
	case reflect.String:
		return encodeString(dst, rv.String())
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		return encodeInt64(dst, rv.Int()), nil
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		return appendHead(dst, 0, rv.Uint()), nil
	case reflect.Interface, reflect.Pointer:
		if rv.IsNil() {
			return append(dst, 0xf6), nil
		}
		if rv.CanInterface() {
			if val, ok := rv.Interface().(Valuer); ok {
				return encodeItem(dst, val.ToValue(), depth)
			}
		}
		return encodeItem(dst, rv.Elem().Interface(), depth)
	case reflect.Slice, reflect.Array:
		if rv.Type().Elem().Kind() == reflect.Uint8 {
			return nil, profileErr("byte strings are forbidden")
		}
		n := rv.Len()
		dst = appendHead(dst, 4, uint64(n))
		var err error
		for i := 0; i < n; i++ {
			if dst, err = encodeItem(dst, rv.Index(i).Interface(), depth+1); err != nil {
				return nil, err
			}
		}
		return dst, nil
	case reflect.Map:
		if rv.Type().Key().Kind() != reflect.String {
			return nil, profileErr("map keys must be text")
		}
		return encodeMap(dst, rv.Len(), func(yield func(string, any)) {
			it := rv.MapRange()
			for it.Next() {
				yield(it.Key().String(), it.Value().Interface())
			}
		}, depth)
	}
	return nil, profileErr("value of type %s is outside the CBOR profile", rv.Type())
}

func encodeMap(dst []byte, n int, each func(func(string, any)), depth int) ([]byte, error) {
	type pair struct{ key, val []byte }
	pairs := make([]pair, 0, n)
	var first error
	each(func(k string, e any) {
		if first != nil {
			return
		}
		kb, err := encodeString(nil, k)
		if err != nil {
			first = err
			return
		}
		vb, err := encodeItem(nil, e, depth+1)
		if err != nil {
			first = err
			return
		}
		pairs = append(pairs, pair{kb, vb})
	})
	if first != nil {
		return nil, first
	}
	sort.Slice(pairs, func(i, j int) bool { return bytes.Compare(pairs[i].key, pairs[j].key) < 0 })
	dst = appendHead(dst, 5, uint64(len(pairs)))
	for _, p := range pairs {
		dst = append(dst, p.key...)
		dst = append(dst, p.val...)
	}
	return dst, nil
}

// DecodeProfile strictly decodes exactly one profile item.
func DecodeProfile(raw []byte) (any, error) {
	d := decoder{raw: raw}
	v, err := d.item(0)
	if err != nil {
		return nil, err
	}
	if d.pos != len(raw) {
		return nil, profileErr("trailing data")
	}
	return v, nil
}

type decoder struct {
	raw []byte
	pos int
}

func (d *decoder) remaining() uint64 { return uint64(len(d.raw) - d.pos) }

func (d *decoder) item(depth int) (any, error) {
	if depth > MaxDepth {
		return nil, profileErr("nesting limit")
	}
	if d.pos >= len(d.raw) {
		return nil, profileErr("truncated item")
	}
	head := d.raw[d.pos]
	d.pos++
	major, ai := head>>5, head&31
	if major == 2 || major == 6 || ai >= 28 {
		return nil, profileErr("forbidden CBOR type or indefinite length")
	}
	if major == 7 {
		switch ai {
		case 20:
			return false, nil
		case 21:
			return true, nil
		case 22:
			return nil, nil
		}
		return nil, profileErr("forbidden simple value or float")
	}
	size := uint64(ai)
	if ai >= 24 {
		width := 1 << (ai - 24)
		if uint64(width) > d.remaining() {
			return nil, profileErr("truncated head")
		}
		size = 0
		for _, b := range d.raw[d.pos : d.pos+width] {
			size = size<<8 | uint64(b)
		}
		d.pos += width
		var least uint64 = 24
		switch width {
		case 2:
			least = 1 << 8
		case 4:
			least = 1 << 16
		case 8:
			least = 1 << 32
		}
		if size < least {
			return nil, profileErr("non-shortest head")
		}
	}
	switch major {
	case 0:
		return size, nil
	case 1:
		if size >= 1<<63 {
			return nil, profileErr("negative integer outside profile")
		}
		return -1 - int64(size), nil
	case 3:
		if size > d.remaining() {
			return nil, profileErr("truncated text")
		}
		s := d.raw[d.pos : d.pos+int(size)]
		if !utf8.Valid(s) {
			return nil, profileErr("invalid UTF-8")
		}
		d.pos += int(size)
		return string(s), nil
	case 4:
		if size > d.remaining() {
			return nil, profileErr("truncated array")
		}
		out := make([]any, 0, size)
		for i := uint64(0); i < size; i++ {
			e, err := d.item(depth + 1)
			if err != nil {
				return nil, err
			}
			out = append(out, e)
		}
		return out, nil
	default: // 5
		if size > d.remaining()/2 {
			return nil, profileErr("truncated map")
		}
		out := make(map[string]any, size)
		var prev []byte
		for i := uint64(0); i < size; i++ {
			start := d.pos
			if d.pos >= len(d.raw) {
				return nil, profileErr("truncated map")
			}
			if d.raw[d.pos]>>5 != 3 {
				return nil, profileErr("non-text map key")
			}
			k, err := d.item(depth + 1)
			if err != nil {
				return nil, err
			}
			enc := d.raw[start:d.pos]
			if prev != nil && bytes.Compare(enc, prev) <= 0 {
				return nil, profileErr("duplicate or unsorted map key")
			}
			prev = enc
			e, err := d.item(depth + 1)
			if err != nil {
				return nil, err
			}
			out[k.(string)] = e
		}
		return out, nil
	}
}
