package types_test

import (
	"bytes"
	"errors"
	"strings"
	"testing"

	"github.com/softmax-universe/coglet/sdk/surface-go/codec"
	"github.com/softmax-universe/coglet/sdk/surface-go/types"
)

var (
	id   = strings.Repeat("c", 32)
	corr = strings.Repeat("b", 32)
)

func TestTypedEventRoundTrip(t *testing.T) {
	ev := types.Event{
		Source:        "surface:" + strings.Repeat("a", 64),
		Kind:          "speech.partial",
		Seq:           1<<64 - 1,
		LogicalTime:   42,
		Payload:       types.ProfileMap{"n": int64(-1 << 63), "list": []any{true, nil, "x"}},
		CorrelationId: corr,
	}
	raw, err := ev.ToCBOR()
	if err != nil {
		t.Fatal(err)
	}
	back, err := types.DecodeEvent(raw)
	if err != nil {
		t.Fatal(err)
	}
	again, err := back.ToCBOR()
	if err != nil || !bytes.Equal(raw, again) || back.Seq != ev.Seq {
		t.Fatalf("round trip: %x %x %v", raw, again, err)
	}
	bad := ev
	bad.Kind = "NotACapability"
	if _, err := bad.ToCBOR(); !errors.Is(err, codec.ErrShape) {
		t.Fatalf("constraint not enforced: %v", err)
	}
}

func TestTypedUnionAndOptional(t *testing.T) {
	details := types.ProfileMap{"why": "because"}
	reject := types.ActionAccept2{
		Id: id, CorrelationId: corr, Accepted: false,
		Error: types.Error{Code: "permission_denied", Message: "Grant missing", Retryable: false, Details: &details},
	}
	raw, err := reject.ToCBOR()
	if err != nil {
		t.Fatal(err)
	}
	got, err := types.DecodeActionAccept(raw)
	if err != nil {
		t.Fatal(err)
	}
	v, ok := got.(types.ActionAccept2)
	if !ok || v.Error.Details == nil || (*v.Error.Details)["why"] != "because" {
		t.Fatalf("wrong branch: %#v", got)
	}
	if _, err := types.DecodeActionAccept1(raw); !errors.Is(err, codec.ErrShape) {
		t.Fatalf("branch 1 accepted branch 2 item: %v", err)
	}
	// A branch-1 struct holding accepted=false must not encode.
	if _, err := (types.ActionAccept1{Id: id, CorrelationId: corr, Accepted: false}).ToCBOR(); err == nil {
		t.Fatal("variant literal not enforced")
	}
	// Generic API accepts typed values and returns the generic model.
	generic, err := types.Decode("action-accept", raw)
	if err != nil {
		t.Fatal(err)
	}
	if again, err := types.Encode("action-accept", generic); err != nil || !bytes.Equal(again, raw) {
		t.Fatalf("generic round trip: %v", err)
	}
	if _, err := types.Decode("no-such-rule", raw); err == nil {
		t.Fatal("unknown rule accepted")
	}
}
