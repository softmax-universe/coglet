//go:build linux

// The conformance host is a no-hardware semantic renderer using the public SDK.
// It reads supervisor fixture configuration, never case expected responses.
package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync/atomic"
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

// provision reads the trusted local contract profile and optional pinned catalog from the
// supervisor environment (ADR-0030 1a). Absent selector means surface-0.1; a present empty or
// unknown one fails closed in surface.New (Go's "" spelling is only for omitted Config).
func provision() (profile string, catalog *surface.TrustedCatalog, err error) {
	profile = surface.ProfileSurface01
	if v, ok := os.LookupEnv("COGLET_CONFORMANCE_CONTRACT_PROFILE"); ok {
		if v == "" {
			return "", nil, errors.New("empty contract profile")
		}
		profile = v
	}
	path, hasPath := os.LookupEnv("COGLET_FIXTURE_CATALOG")
	pin, hasPin := os.LookupEnv("COGLET_FIXTURE_CATALOG_SHA256")
	if hasPath != hasPin {
		return "", nil, errors.New("catalog path and sha256 pin must be supplied together")
	}
	if hasPath {
		raw, err := os.ReadFile(path)
		if err != nil {
			return "", nil, err
		}
		if catalog, err = surface.NewTrustedCatalog(raw, pin); err != nil {
			return "", nil, err
		}
	}
	return profile, catalog, nil
}

// builtinGrants drops the test app's operations and private resource (downgrade path).
func builtinGrants(in []types.Grant) []types.Grant {
	out := []types.Grant{}
	for _, g := range in {
		g.Operations = append([]string{}, g.Operations...)
		g.Resources = append([]string{}, g.Resources...)
		ops, res := []string{}, []string{}
		for _, o := range g.Operations {
			if !strings.HasPrefix(o, "softmax.app.") {
				ops = append(ops, o)
			}
		}
		for _, r := range g.Resources {
			if !strings.HasPrefix(r, "softmax.") {
				res = append(res, r)
			}
		}
		g.Operations, g.Resources = ops, res
		out = append(out, g)
	}
	return out
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
	// Test-only logical clock (ADR-0021): when the harness provides a control socket the
	// surface time is exactly the advances it sends, never the wall clock.
	var logical, base atomic.Uint64
	control, err := fixtureControl()
	if err != nil {
		return err
	}
	elapsed := func() uint64 {
		if control > 0 {
			return logical.Load() - base.Load()
		}
		return uint64(time.Since(started).Microseconds())
	}
	clock := func() uint64 {
		if !negotiated {
			return f.Clock
		}
		return f.Clock + elapsed()
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
	profile, trusted, err := provision()
	if err != nil {
		return err
	}
	s, err := surface.New(surface.Config{Identity: f.Surface, Coglets: ids, Ledger: filepath.Join(dir, "ledger"), Create: true, Clock: clock, Stop: stop, EpochFloor: epoch - 1,
		ContractProfile: profile, TrustedCatalog: trusted})
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
			}
			if strings.HasPrefix(name, "softmax.app.") {
				// Test-only stand-in for an app: one fixed title, no real app.
				if strings.HasSuffix(name, ".state") {
					return map[string]any{"title": "hello"}, nil
				}
				return map[string]any{}, nil
			}
			switch name {
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
		if strings.HasPrefix(name, "softmax.app.") {
			prepare = func(map[string]any, surface.RequestContext) error { return nil }
		}
		op, err := s.BoundOperation(name, prepare, execute)
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
			err := s.Grant(f.Grant.Entries)
			if err != nil && profile == surface.ProfileSurface02 {
				// A 0.1 peer on a surface-0.2 provisioning cannot be granted catalog operations:
				// the supervisor then grants only built-ins (the SDK has no negotiated-version
				// accessor yet, so the adapter retries instead of inspecting it).
				err = s.Grant(builtinGrants(f.Grant.Entries))
			}
			if err != nil {
				return err
			}
			started = time.Now()
			base.Store(logical.Load())
			negotiated = true
			return nil
		},
		Control: control,
		Advance: func(micros uint64) error {
			if micros < logical.Load() {
				return errors.New("invalid fixture clock advance")
			}
			logical.Store(micros)
			return nil
		},
		Now: logical.Load,
		Poll: func() error {
			if clock() >= until {
				stop()
			}
			if negotiated && killAfter > 0 && !killed && time.Duration(elapsed())*time.Microsecond >= killAfter {
				killed = true
				return s.Kill("local operator off-switch")
			}
			return nil
		},
	})
}
