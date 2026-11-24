package surface

import (
	_ "embed"
	"encoding/json"
	"errors"
	"strings"

	"github.com/softmax-universe/coglet/sdk/surface-go/codec"
	"github.com/softmax-universe/coglet/sdk/surface-go/types"
)

//go:embed bindings.json
var catalogJSON []byte

type Binding struct {
	Entry, Direction, Input string
	Output                  *string
}

var catalog = func() map[string]Binding {
	var c struct{ Operations map[string]Binding }
	if err := json.Unmarshal(catalogJSON, &c); err != nil {
		panic(err)
	}
	return c.Operations
}()

// Operation declares a release-pinned operation. Prepare must be side-effect
// free. Execute is bounded/nonblocking; nil result means asynchronous admission.
// Complete must be called later, not reentrantly from a callback. Capture the
// immutable RequestContext and use Effect for every asynchronous native effect.
// Device stream/session ownership and semantic media normalization belong to the
// adapter; do not advertise a speech operation without implementing its contract.
type Operation struct {
	Name      string
	Resources []string
	Prepare   func(map[string]any, RequestContext) error
	Execute   func(map[string]any, RequestContext) (map[string]any, error)
}

// BoundOperation selects a built-in binding and its minimum private resources.
// Register only the returned operations your adapter actually implements.
func BoundOperation(name string, prepare func(map[string]any, RequestContext) error, execute func(map[string]any, RequestContext) (map[string]any, error)) (Operation, error) {
	if _, ok := catalog[name]; !ok {
		return Operation{}, errors.New("unknown operation binding")
	}
	return Operation{Name: name, Resources: requiredResources(name), Prepare: prepare, Execute: execute}, nil
}

type registered struct {
	Operation
	binding Binding
}

// Rejection is a safe common error; arbitrary callback errors become internal.
type Rejection struct{ Code string }

func (r *Rejection) Error() string { return r.Code }
func Refuse(code string) error     { return &Rejection{code} }
func errorValue(code string) map[string]any {
	return map[string]any{"code": code, "message": code, "retryable": false}
}
func errorCode(err error) string {
	var r *Rejection
	if errors.As(err, &r) && types.Validate("common::error", errorValue(r.Code)) == nil {
		return r.Code
	}
	return "internal"
}
func rule(s string) string { return strings.Replace(s, ":", "::", 1) }
func (b Binding) uri() string {
	s := b.Input
	if b.Direction == "read" && b.Output != nil {
		s = *b.Output
	}
	p := strings.SplitN(s, ":", 2)
	return "https://coglet.dev/spec/messages/" + p[0] + ".schema.json#/$defs/" + p[1]
}
func requiredResources(name string) []string {
	switch name {
	case "expression.set_expression":
		return []string{"display.face"}
	case "speech.start", "speech.partial", "speech.end":
		return []string{"audio.mic"}
	case "speech.position", "speech.speak_chunk", "speech.stop", "speech.backchannel", "speech.heard":
		return []string{"audio.speaker"}
	case "speech.barge_in", "speech.session_open", "speech.session_close", "speech.tool_call", "speech.tool_result", "speech.work_state", "speech.work_intent", "speech.work_done", "speech.work_result":
		return []string{"audio.mic", "audio.speaker"}
	}
	return nil
}
func includes(all []string, need string) bool {
	for _, v := range all {
		if v == need {
			return true
		}
	}
	return false
}
func semantic(name string, v map[string]any) error {
	limit := 0
	switch name {
	case "speech.speak_chunk":
		limit = 4096
	case "speech.partial", "speech.end", "speech.heard":
		limit = 16384
	}
	if text, ok := v["text"].(string); ok && limit > 0 && len(text) > limit {
		return Refuse("invalid_argument")
	}
	for key, max := range map[string]int{"arguments": 8192, "value": 32000} {
		if strings.HasPrefix(name, "speech.work_") {
			if data, ok := v[key]; ok {
				raw, err := codec.EncodeProfile(data)
				if err != nil || len(raw) > max {
					return Refuse("invalid_argument")
				}
			}
		}
	}
	return nil
}
