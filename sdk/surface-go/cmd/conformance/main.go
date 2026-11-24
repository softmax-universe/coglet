//go:build linux

// The conformance host is a no-hardware semantic renderer using the public SDK.
// It reads supervisor fixture configuration, never case expected responses.
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"time"

	surface "github.com/softmax-universe/coglet/sdk/surface-go"
	"github.com/softmax-universe/coglet/sdk/surface-go/types"
)

type fixture struct {
	Surface string
	Clock   uint64
	Hello   struct {
		Body struct {
			CorrelationID string `json:"correlation_id"`
		}
	}
	Grant        struct{ Entries []types.Grant }
	SpeechConfig map[string]any    `json:"speech_config"`
	Startup      map[string]uint64 `json:"startup_epoch_by_case"`
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
func run() error {
	raw, err := os.ReadFile(os.Getenv("COGLET_FIXTURE_CONFIG"))
	if err != nil {
		return err
	}
	var f fixture
	if err = json.Unmarshal(raw, &f); err != nil {
		return err
	}
	dir, err := os.MkdirTemp("", "coglet-go-")
	if err != nil {
		return err
	}
	defer os.RemoveAll(dir)
	started := time.Now()
	negotiated := false
	clock := func() uint64 {
		if !negotiated {
			return f.Clock
		}
		return f.Clock + uint64(time.Since(started).Microseconds())
	}
	expression := "neutral"
	intensity := uint64(0)
	until := uint64(0)
	stop := func() { expression = "neutral"; intensity = 0; until = 0 }
	ids := []string{}
	names := map[string]bool{}
	for _, g := range f.Grant.Entries {
		ids = append(ids, g.Coglet)
		for _, n := range g.Operations {
			names[n] = true
		}
	}
	epoch := f.Startup[os.Getenv("COGLET_CONFORMANCE_CASE")]
	if epoch == 0 {
		epoch = 1
	}
	s, err := surface.New(surface.Config{Identity: f.Surface, Coglets: ids, Ledger: filepath.Join(dir, "ledger"), Create: true, Clock: clock, Stop: stop, EpochFloor: epoch - 1})
	if err != nil {
		return err
	}
	defer s.Close()
	sorted := []string{}
	for n := range names {
		sorted = append(sorted, n)
	}
	sort.Strings(sorted)
	for _, name := range sorted {
		name := name
		prepare := func(args map[string]any, _ surface.RequestContext) error {
			switch name {
			case "expression.set_expression", "expression.state", "speech.config":
				return nil
			}
			if _, ok := args["stream"]; ok {
				return surface.Refuse("not_found")
			}
			if _, ok := args["session"]; ok {
				return surface.Refuse("not_found")
			}
			return surface.Refuse("unavailable")
		}
		execute := func(args map[string]any, _ surface.RequestContext) (map[string]any, error) {
			n := clock()
			switch name {
			case "speech.config":
				return f.SpeechConfig, nil
			case "expression.set_expression":
				d := args["duration_ms"].(uint64)
				if d == 0 {
					stop()
				} else {
					expression = args["expression"].(string)
					intensity = args["intensity"].(uint64)
					until = n + d*1000
				}
				return map[string]any{}, nil
			case "expression.state":
				if n >= until {
					stop()
				}
				remaining := uint64(0)
				if until > n {
					remaining = (until - n + 999) / 1000
				}
				return map[string]any{"expression": expression, "intensity": intensity, "remaining_ms": remaining}, nil
			}
			return nil, surface.Refuse("unavailable")
		}
		op, err := surface.BoundOperation(name, prepare, execute)
		if err != nil {
			return err
		}
		if err = s.Register(op); err != nil {
			return err
		}
	}
	killMS := os.Getenv("COGLET_FIXTURE_KILL_AT_MS")
	killAfter := time.Duration(0)
	if killMS != "" {
		if _, err = fmt.Sscanf(killMS, "%d", &killAfter); err != nil {
			return err
		}
		killAfter *= time.Millisecond
	}
	killed := false
	return surface.ServeFD(context.Background(), s, int(os.Stdin.Fd()), int(os.Stdout.Fd()), surface.ServeOptions{
		HelloID: f.Hello.Body.CorrelationID,
		Ready:   func() { fmt.Fprintln(os.Stderr, "COGLET_CONFORMANCE_READY") },
		Configured: func() error {
			if err := s.Grant(f.Grant.Entries); err != nil {
				return err
			}
			started = time.Now()
			negotiated = true
			return nil
		},
		Poll: func() error {
			if clock() >= until {
				stop()
			}
			if negotiated && killAfter > 0 && !killed && time.Since(started) >= killAfter {
				killed = true
				return s.Kill("local operator off-switch")
			}
			return nil
		},
	})
}
