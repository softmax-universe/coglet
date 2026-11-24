// Package surface implements the supervisor-bound Coglet surface protocol 0.1
// on the generated types and strict deterministic CBOR codec. It is not a hub
// enrollment client: local OS peer authentication precedes surface negotiation.
package surface

import (
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"math"
	"sort"
	"sync"
	"sync/atomic"
	"time"

	"github.com/softmax-universe/coglet/sdk/surface-go/codec"
	"github.com/softmax-universe/coglet/sdk/surface-go/types"
)

const MaxFrame = 1_048_576
const wire = "surface::surface-message"
const retention = uint64(24 * 60 * 60 * 1_000_000)

// Config is trusted supervisor configuration, never a claimed wire identity.
// Clock returns persistent-epoch logical microseconds and must be thread safe.
// Stop is an idempotent native reflex, called before opening any state. It must
// not call Surface methods. Neither callbacks nor storage may be audio-thread work.
type Config struct {
	Identity string
	Coglets  []string
	Ledger   string
	Create   bool
	// EpochFloor is the externally retained supervisor high-water mark; it can
	// only raise the durable epoch, never restore grants or a missing ledger.
	EpochFloor uint64
	Clock      func() uint64
	Stop       func()
}

// RequestContext is immutable authority captured at admission. ItemID is the
// original wire action ID, not a provider/native cancellation token.
type RequestContext struct {
	Coglet, ItemID, CorrelationID string
	Epoch, Deadline               uint64
	generation, expires           uint64
	surface                       *Surface
}

// Effect atomically checks the kill/generation/deadline fence with native work.
// fn must be bounded; asynchronous native callbacks retain this exact context.
func (c RequestContext) Effect(fn func() error) error {
	if c.surface == nil {
		return Refuse("permission_denied")
	}
	s := c.surface
	s.effect.Lock()
	defer s.effect.Unlock()
	now := s.clock()
	if s.killed.Load() || s.generation.Load() != c.generation || now >= c.expires {
		return Refuse("permission_denied")
	}
	if now >= c.Deadline {
		return Refuse("deadline_exceeded")
	}
	return fn()
}

// Surface serializes control operations. Native effects use a separate lock so
// Kill stops I/O without waiting for storage or a blocked control callback.
type Surface struct {
	mu                                                       sync.Mutex
	effect                                                   sync.Mutex
	killed                                                   atomic.Bool
	generation                                               atomic.Uint64
	clock                                                    func() uint64
	stop                                                     func()
	ledger                                                   *ledger
	identity                                                 string
	coglets                                                  map[string]bool
	operations                                               map[string]registered
	grants                                                   []types.Grant
	queue                                                    []queuedFrame
	state, helloID                                           string
	revision, lastClock, lastReceived, lastHealth, peerClock uint64
	reads                                                    map[string]bool
	closed                                                   bool
}

func ID() string {
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		panic(err)
	}
	return hex.EncodeToString(b[:])
}
func New(c Config) (*Surface, error) {
	if c.Stop == nil {
		return nil, errors.New("native Stop required")
	}
	c.Stop()
	if err := types.Validate("common::surface-id", c.Identity); err != nil {
		return nil, err
	}
	if c.Clock == nil {
		start := time.Now()
		base := uint64(start.UnixMicro())
		c.Clock = func() uint64 { return base + uint64(time.Since(start).Microseconds()) }
	}
	s := &Surface{identity: c.Identity, coglets: map[string]bool{}, operations: map[string]registered{}, state: "closed", clock: c.Clock, stop: c.Stop, revision: 1, reads: map[string]bool{}}
	for _, id := range c.Coglets {
		if err := types.Validate("common::coglet-id", id); err != nil {
			return nil, err
		}
		s.coglets[id] = true
	}
	l, err := openLedger(c.Ledger, c.Identity, c.Create)
	if err != nil {
		return nil, err
	}
	s.ledger = l
	if l.data.Epoch < c.EpochFloor {
		l.data.Epoch = c.EpochFloor
	}
	s.lastClock = l.data.Clock
	if l.data.Running {
		l.data.Killed = true
	}
	s.killed.Store(l.data.Killed)
	// Validate retained envelopes/receipts before exposing any authority. Recovery
	// records failed/unavailable for uncertain accepted work, never re-executes it.
	for key, r := range l.data.Actions {
		a, e := actionFrom(r.Envelope)
		if e != nil || key != actionKey(a) {
			l.close()
			return nil, errors.New("corrupt action ledger")
		}
		v, e := types.Decode("common::action-accept", r.Accept)
		if e != nil {
			l.close()
			return nil, e
		}
		receipt := v.(map[string]any)
		if receipt["id"] != a["id"] || receipt["correlation_id"] != a["correlation_id"] {
			l.close()
			return nil, errors.New("receipt identity mismatch")
		}
		if r.Outcome != nil {
			if _, e = types.Decode("common::action-outcome", r.Outcome); e != nil {
				l.close()
				return nil, e
			}
		}
		if receipt["accepted"] == true && r.Outcome == nil {
			r.Outcome, _ = types.Encode("common::action-outcome", outcome(a, "failed", nil, "unavailable"))
			r.End = s.clock()
		}
	}
	if _, err = s.now(); err == nil {
		err = l.save()
	}
	if err != nil {
		l.close()
		return nil, err
	}
	return s, nil
}
func (s *Surface) now() (uint64, error) {
	n := s.clock()
	if n < s.lastClock {
		return 0, errors.New("clock rollback")
	}
	s.lastClock = n
	s.ledger.data.Clock = n
	return n, nil
}
func (s *Surface) save() error { return s.ledger.save() }
func (s *Surface) stopNative() {
	s.generation.Add(1)
	s.effect.Lock()
	defer s.effect.Unlock()
	s.stop()
}
func (s *Surface) guard(err *error) {
	if *err != nil {
		if e := s.abort(); e != nil {
			*err = errors.Join(*err, e)
		}
	}
}
func (s *Surface) abort() error {
	s.stopNative()
	s.state = "closed"
	s.ledger.data.Running = false
	s.grants = nil
	s.queue = nil
	for _, r := range s.ledger.data.Actions {
		if accepted(r) && r.Outcome == nil {
			a, _ := actionFrom(r.Envelope)
			r.Outcome, _ = types.Encode("common::action-outcome", outcome(a, "cancelled", nil, "unavailable"))
			r.End = s.lastClock
		}
	}
	return s.save()
}
func (s *Surface) Register(op Operation) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	b, ok := catalog[op.Name]
	if !ok || s.state != "closed" || s.closed {
		return errors.New("unknown binding or invalid lifecycle")
	}
	if _, ok = s.operations[op.Name]; ok {
		return errors.New("duplicate operation")
	}
	if b.Direction != "event" && op.Execute == nil {
		return errors.New("operation needs Execute")
	}
	for _, r := range requiredResources(op.Name) {
		if !includes(op.Resources, r) {
			return errors.New("missing binding resource")
		}
	}
	for _, r := range op.Resources {
		if err := types.Validate("common::capability-name", r); err != nil {
			return err
		}
	}
	op.Resources = append([]string{}, op.Resources...)
	s.operations[op.Name] = registered{op, b}
	return nil
}
func (s *Surface) Open(cid string) (raw []byte, err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	defer s.guard(&err)
	if s.closed || s.state != "closed" {
		return nil, errors.New("already open or closed permanently")
	}
	if cid == "" {
		cid = ID()
	}
	n, err := s.now()
	if err != nil {
		return nil, err
	}
	s.helloID = cid
	s.reads = map[string]bool{}
	s.revision = 1
	s.peerClock = 0
	s.lastReceived = n
	s.lastHealth = n
	raw, err = message("hello", map[string]any{"surface": s.identity, "offer": map[string]any{"versions": []any{"0.1"}}, "clock": n, "correlation_id": cid})
	if err != nil {
		return nil, err
	}
	s.state = "negotiating"
	s.ledger.data.Running = true
	err = s.save()
	return
}
func message(kind string, body any) ([]byte, error) {
	raw, err := types.Encode(wire, map[string]any{"type": kind, "body": body})
	if err == nil && (len(raw) == 0 || len(raw) > MaxFrame) {
		err = errors.New("invalid frame size")
	}
	return raw, err
}
func (s *Surface) emit(kind string, body any) error {
	raw, err := message(kind, body)
	if err != nil {
		return err
	}
	// Reserve two slots for kill + stopped health, and one terminal delivery
	// for every pending admission. A cancellation consumes its reserved slot.
	if len(s.queue)+s.pending() >= 126 {
		return errors.New("resource_exhausted: output queue")
	}
	s.queue = append(s.queue, queuedFrame{raw, s.generation.Load()})
	return nil
}

// Drain must be delivered on the same control owner as Receive/Tick/Grant. Serve
// does this automatically; custom transports must purge private queued frames on
// revocation and abort if a revoked private frame was only partially written.
func (s *Surface) Drain() [][]byte {
	s.mu.Lock()
	defer s.mu.Unlock()
	q := make([][]byte, 0, len(s.queue))
	for _, f := range s.queue {
		q = append(q, f.raw)
	}
	s.queue = nil
	return q
}

type queuedFrame struct {
	raw        []byte
	generation uint64
}

func (s *Surface) drainFrames() []queuedFrame {
	s.mu.Lock()
	defer s.mu.Unlock()
	q := s.queue
	s.queue = nil
	return q
}
func (s *Surface) Ready() bool   { s.mu.Lock(); defer s.mu.Unlock(); return s.state == "ready" }
func (s *Surface) Epoch() uint64 { s.mu.Lock(); defer s.mu.Unlock(); return s.ledger.data.Epoch }
func (s *Surface) capabilities() error {
	entries := map[string]map[string]any{}
	names := make([]string, 0, len(s.operations))
	for n := range s.operations {
		names = append(names, n)
	}
	sort.Strings(names)
	for _, name := range names {
		op := s.operations[name]
		b := op.binding
		e := entries[b.Entry]
		if e == nil {
			e = map[string]any{"name": b.Entry, "events": []any{}, "reads": []any{}, "actions": []any{}, "exclusive": []any{}, "schemas": map[string]any{}}
			entries[b.Entry] = e
		}
		key := map[string]string{"event": "events", "read": "reads", "action": "actions"}[b.Direction]
		e[key] = append(e[key].([]any), name)
		e["schemas"].(map[string]any)[name] = b.uri()
		for _, r := range op.Resources {
			found := false
			for _, v := range e["exclusive"].([]any) {
				if v == r {
					found = true
				}
			}
			if !found {
				e["exclusive"] = append(e["exclusive"].([]any), r)
			}
		}
	}
	keys := make([]string, 0, len(entries))
	for k := range entries {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	list := []any{}
	for _, k := range keys {
		list = append(list, entries[k])
	}
	n, err := s.now()
	if err != nil {
		return err
	}
	return s.emit("capabilities", map[string]any{"revision": s.revision, "clock": n, "entries": list, "correlation_id": ID()})
}
func (s *Surface) Grant(entries []types.Grant) (err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	defer func() {
		var rejection *Rejection
		if !errors.As(err, &rejection) {
			s.guard(&err)
		}
	}()
	return s.grant(entries)
}
func (s *Surface) grant(entries []types.Grant) error {
	if s.state != "ready" {
		return Refuse("invalid_argument")
	}
	n, err := s.now()
	if err != nil {
		return err
	}
	seen, owners := map[string]bool{}, map[string]bool{}
	copyEntries := make([]types.Grant, 0, len(entries))
	for _, g := range entries {
		if err = types.Validate("surface::grant", g); err != nil {
			return Refuse("invalid_argument")
		}
		if !s.coglets[g.Coglet] || seen[g.Coglet] || g.Expires <= n {
			return Refuse("invalid_argument")
		}
		seen[g.Coglet] = true
		for _, name := range g.Operations {
			if _, ok := s.operations[name]; !ok {
				return Refuse("invalid_argument")
			}
		}
		for _, r := range g.Resources {
			if owners[r] {
				return Refuse("invalid_argument")
			}
			owners[r] = true
		}
		g.Operations = append([]string{}, g.Operations...)
		g.Resources = append([]string{}, g.Resources...)
		copyEntries = append(copyEntries, g)
	}
	if s.ledger.data.Epoch == math.MaxUint64 {
		return errors.New("epoch exhausted")
	}
	if len(s.grants) > 0 {
		s.stopNative()
		if err = s.cancel("cancelled", "cancelled"); err != nil {
			return err
		}
		s.purge(false)
	}
	s.ledger.data.Epoch++
	if s.killed.Load() {
		copyEntries = []types.Grant{}
	}
	s.grants = copyEntries
	if err = s.save(); err != nil {
		return err
	}
	return s.emit("grants", map[string]any{"epoch": s.ledger.data.Epoch, "entries": copyEntries, "correlation_id": ID()})
}
func (s *Surface) Receive(raw []byte) (err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	defer s.guard(&err)
	if s.closed || len(raw) == 0 || len(raw) > MaxFrame {
		return errors.New("invalid frame/lifecycle")
	}
	v, err := types.Decode(wire, raw)
	if err != nil {
		return err
	}
	m := v.(map[string]any)
	b := m["body"].(map[string]any)
	kind := m["type"].(string)
	n, err := s.now()
	if err != nil {
		return err
	}
	s.lastReceived = n
	if s.state == "negotiating" {
		if kind != "hello_reply" || b["correlation_id"] != s.helloID || b["selection"].(map[string]any)["version"] != "0.1" {
			return errors.New("invalid negotiation")
		}
		s.state = "ready"
		return s.capabilities()
	}
	if s.state != "ready" {
		return errors.New("not negotiated")
	}
	switch kind {
	case "act":
		return s.act(raw, b)
	case "read":
		return s.read(b)
	case "health":
		c := b["clock"].(uint64)
		if c < s.peerClock {
			return errors.New("peer clock rollback")
		}
		s.peerClock = c
		return nil
	default:
		return errors.New("wrong direction")
	}
}
func decodeMessage(raw []byte) (map[string]any, error) {
	v, e := types.Decode(wire, raw)
	if e != nil {
		return nil, e
	}
	return v.(map[string]any), nil
}
func actionFrom(raw []byte) (map[string]any, error) {
	v, err := types.Decode(wire, raw)
	if err != nil {
		return nil, err
	}
	m := v.(map[string]any)
	if m["type"] != "act" {
		return nil, errors.New("not act")
	}
	return m["body"].(map[string]any)["action"].(map[string]any), nil
}
func actionKey(a map[string]any) string { return a["coglet"].(string) + "/" + a["id"].(string) }
func accepted(r *record) bool {
	v, e := types.Decode("common::action-accept", r.Accept)
	return e == nil && v.(map[string]any)["accepted"] == true
}
func receipt(a map[string]any, code string) map[string]any {
	m := map[string]any{"id": a["id"], "correlation_id": a["correlation_id"], "accepted": code == ""}
	if code != "" {
		m["error"] = errorValue(code)
	}
	return m
}
func outcome(a map[string]any, status string, result map[string]any, code string) map[string]any {
	m := map[string]any{"id": a["id"], "correlation_id": a["correlation_id"], "status": status}
	if status == "succeeded" {
		m["result"] = result
	} else {
		m["error"] = errorValue(code)
	}
	return m
}
func (s *Surface) admit(a map[string]any, epoch uint64, direction string) (op registered, ctx RequestContext, err error) {
	n, err := s.now()
	if err != nil {
		return op, ctx, err
	}
	if s.killed.Load() || epoch != s.ledger.data.Epoch || !s.coglets[a["coglet"].(string)] {
		return op, ctx, Refuse("permission_denied")
	}
	var grant *types.Grant
	for i := range s.grants {
		if s.grants[i].Coglet == a["coglet"] {
			grant = &s.grants[i]
			break
		}
	}
	name := a["capability"].(string)
	if grant == nil || grant.Expires <= n || !includes(grant.Operations, name) {
		return op, ctx, Refuse("permission_denied")
	}
	op, ok := s.operations[name]
	if !ok || op.binding.Direction != direction {
		return op, ctx, Refuse("invalid_argument")
	}
	for _, r := range op.Resources {
		if !includes(grant.Resources, r) {
			return op, ctx, Refuse("permission_denied")
		}
	}
	d := a["deadline"].(uint64)
	if d <= n {
		return op, ctx, Refuse("deadline_exceeded")
	}
	if d-n > 60_000_000 {
		return op, ctx, Refuse("invalid_argument")
	}
	args := a["args"].(map[string]any)
	if types.Validate(rule(op.binding.Input), args) != nil {
		return op, ctx, Refuse("invalid_argument")
	}
	if e := semantic(name, args); e != nil {
		return op, ctx, e
	}
	ctx = RequestContext{Coglet: a["coglet"].(string), Epoch: epoch, Deadline: d, CorrelationID: a["correlation_id"].(string), generation: s.generation.Load(), expires: grant.Expires, surface: s}
	ctx.ItemID, _ = a["id"].(string)
	if op.Prepare != nil {
		if err = op.Prepare(args, ctx); err != nil {
			return op, ctx, err
		}
	}
	err = ctx.Effect(func() error { return nil })
	return
}
func (s *Surface) act(raw []byte, b map[string]any) error {
	a := b["action"].(map[string]any)
	key := actionKey(a)
	if !s.coglets[a["coglet"].(string)] {
		return s.emit("accept", receipt(a, "permission_denied"))
	}
	s.prune()
	if old := s.ledger.data.Actions[key]; old != nil {
		if !bytes.Equal(old.Envelope, raw) {
			return s.emit("accept", receipt(a, "conflict"))
		}
		v, _ := types.Decode("common::action-accept", old.Accept)
		if err := s.emit("accept", v); err != nil {
			return err
		}
		if old.Outcome != nil {
			return s.terminalEvent(old.Outcome)
		}
		return nil
	}
	op, ctx, e := s.admit(a, b["grant_epoch"].(uint64), "action")
	code := ""
	if e != nil {
		var r *Rejection
		if !errors.As(e, &r) {
			return e
		}
		code = errorCode(e)
	}
	if code == "" && (s.pending() >= 64 || len(s.queue)+s.pending()+2 > 126) {
		code = "resource_exhausted"
	}
	if len(s.ledger.data.Actions) >= 4096 {
		return errors.New("resource_exhausted: receipt retention")
	}
	accept := receipt(a, code)
	enc, err := types.Encode("common::action-accept", accept)
	if err != nil {
		return err
	}
	r := &record{Envelope: append([]byte{}, raw...), Accept: enc}
	if code != "" {
		r.End = s.lastClock
	}
	s.ledger.data.Actions[key] = r
	if err = s.save(); err != nil {
		return err
	}
	if err = s.emit("accept", accept); err != nil {
		return err
	}
	if code != "" {
		return nil
	}
	var result map[string]any
	err = ctx.Effect(func() error { var e error; result, e = op.Execute(a["args"].(map[string]any), ctx); return e })
	if err != nil {
		status, code := "failed", errorCode(err)
		if code == "permission_denied" {
			status = "cancelled"
		}
		if code == "deadline_exceeded" {
			status = "expired"
		}
		s.stopNative()
		_, e = s.complete(ctx, status, nil, code)
		return e
	}
	if result != nil {
		_, err = s.complete(ctx, "succeeded", result, "")
	}
	return err
}

// Complete records exactly one terminal result. Late/duplicate completions return
// false. A success at the exclusive deadline becomes expired, even without Tick.
func (s *Surface) Complete(ctx RequestContext, status string, result map[string]any, code string) (ok bool, err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	defer s.guard(&err)
	if s.closed {
		return false, errors.New("closed")
	}
	return s.complete(ctx, status, result, code)
}
func (s *Surface) complete(ctx RequestContext, status string, result map[string]any, code string) (bool, error) {
	if ctx.surface != s {
		return false, Refuse("permission_denied")
	}
	r := s.ledger.data.Actions[ctx.Coglet+"/"+ctx.ItemID]
	if r == nil || !accepted(r) || r.Outcome != nil {
		return false, nil
	}
	a, err := actionFrom(r.Envelope)
	if err != nil {
		return false, err
	}
	n, err := s.now()
	if err != nil {
		return false, err
	}
	if status == "succeeded" {
		if e := ctx.Effect(func() error { return nil }); e != nil {
			code = errorCode(e)
			status = "cancelled"
			if code == "deadline_exceeded" {
				status = "expired"
			}
			s.stopNative()
		}
	}
	if status == "succeeded" {
		op := s.operations[a["capability"].(string)]
		if op.binding.Output == nil {
			return false, errors.New("missing output binding")
		}
		if err = types.Validate(rule(*op.binding.Output), result); err != nil {
			return false, err
		}
		if err = semantic(op.Name, result); err != nil {
			return false, err
		}
	}
	raw, err := types.Encode("common::action-outcome", outcome(a, status, result, code))
	if err != nil {
		return false, err
	}
	r.Outcome = raw
	r.End = n
	if err = s.save(); err != nil {
		return false, err
	}
	return true, s.terminalEvent(raw)
}
func (s *Surface) eventFrame(name string, payload any, cid string) error {
	n, err := s.now()
	if err != nil {
		return err
	}
	if s.ledger.data.Seq == math.MaxUint64 {
		return errors.New("event sequence exhausted")
	}
	seq := s.ledger.data.Seq
	s.ledger.data.Seq++
	if err = s.save(); err != nil {
		return err
	}
	return s.emit("event", map[string]any{"source": s.identity, "kind": name, "seq": seq, "logical_time": n, "payload": payload, "correlation_id": cid})
}
func (s *Surface) terminalEvent(raw []byte) error {
	v, e := types.Decode("common::action-outcome", raw)
	if e != nil {
		return e
	}
	return s.eventFrame("action.outcome", v, v.(map[string]any)["correlation_id"].(string))
}

// Event emits an observation using the context captured when native work began.
// Adapters additionally fence session IDs and enforce owner-scoped item tables.
func (s *Surface) Event(ctx RequestContext, name string, payload map[string]any) (err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed || s.state != "ready" || ctx.surface != s {
		return Refuse("permission_denied")
	}
	if err = ctx.Effect(func() error { return nil }); err != nil {
		return err
	}
	op, ok := s.operations[name]
	if !ok || op.binding.Direction != "event" {
		return Refuse("invalid_argument")
	}
	allowed := false
	for _, g := range s.grants {
		if g.Coglet == ctx.Coglet && includes(g.Operations, name) {
			allowed = true
			for _, r := range op.Resources {
				if !includes(g.Resources, r) {
					allowed = false
				}
			}
		}
	}
	if !allowed {
		return Refuse("permission_denied")
	}
	if err = types.Validate(rule(op.binding.Input), payload); err != nil {
		return err
	}
	if err = semantic(name, payload); err != nil {
		return err
	}
	err = s.eventFrame(name, payload, ID())
	s.guard(&err)
	return err
}

// Context captures owner-scoped authority for unsolicited native capture. It does
// not permit effects after revocation; providers must retain their session ID too.
func (s *Surface) Context(coglet string) (RequestContext, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	n, err := s.now()
	if err != nil {
		return RequestContext{}, err
	}
	for _, g := range s.grants {
		if g.Coglet == coglet && g.Expires > n && s.state == "ready" && !s.killed.Load() {
			return RequestContext{Coglet: coglet, Epoch: s.ledger.data.Epoch, Deadline: g.Expires, expires: g.Expires, generation: s.generation.Load(), surface: s}, nil
		}
	}
	return RequestContext{}, Refuse("permission_denied")
}
func (s *Surface) read(b map[string]any) error {
	cid := b["correlation_id"].(string)
	if s.reads[cid] || len(s.reads) >= 65536 {
		return errors.New("reused or exhausted read correlation")
	}
	s.reads[cid] = true
	op, ctx, err := s.admit(b, b["grant_epoch"].(uint64), "read")
	var result map[string]any
	if err == nil {
		err = ctx.Effect(func() error { var e error; result, e = op.Execute(b["args"].(map[string]any), ctx); return e })
	}
	if err == nil {
		err = types.Validate(rule(*op.binding.Output), result)
	}
	if err == nil {
		err = semantic(op.Name, result)
	}
	if err == nil && op.Name == "speech.heard" && result["item"] != b["args"].(map[string]any)["item"] {
		err = errors.New("heard item mismatch")
	}
	if err == nil {
		err = ctx.Effect(func() error { return nil })
	}
	reply := map[string]any{"correlation_id": cid, "ok": err == nil}
	if err != nil {
		reply["error"] = errorValue(errorCode(err))
	} else {
		if s.ledger.data.Snapshot == math.MaxUint64 {
			return errors.New("snapshot exhausted")
		}
		s.ledger.data.Snapshot++
		if err = s.save(); err != nil {
			return err
		}
		reply["value"] = result
		reply["snapshot"] = s.ledger.data.Snapshot
	}
	return s.emit("read_reply", reply)
}
func (s *Surface) pending() int {
	n := 0
	for _, r := range s.ledger.data.Actions {
		if accepted(r) && r.Outcome == nil {
			n++
		}
	}
	return n
}
func (s *Surface) contextFor(r *record) RequestContext {
	a, _ := actionFrom(r.Envelope)
	return RequestContext{Coglet: a["coglet"].(string), ItemID: a["id"].(string), surface: s}
}
func (s *Surface) cancel(status, code string) error {
	keys := make([]string, 0)
	for key, r := range s.ledger.data.Actions {
		if accepted(r) && r.Outcome == nil {
			keys = append(keys, key)
		}
	}
	sort.Strings(keys)
	for _, key := range keys {
		r := s.ledger.data.Actions[key]
		if _, err := s.complete(s.contextFor(r), status, nil, code); err != nil {
			return err
		}
	}
	return nil
}
func (s *Surface) prune() {
	for k, r := range s.ledger.data.Actions {
		if r.End == 0 {
			continue
		}
		a, _ := actionFrom(r.Envelope)
		end := max(r.End, a["deadline"].(uint64))
		if s.lastClock > end && s.lastClock-end >= retention {
			delete(s.ledger.data.Actions, k)
		}
	}
}
func (s *Surface) health() error {
	n, err := s.now()
	if err != nil {
		return err
	}
	state := "ready"
	if s.killed.Load() {
		state = "stopped"
	}
	return s.emit("health", map[string]any{"state": state, "clock": n, "pending": s.pending(), "correlation_id": ID()})
}
func (s *Surface) Tick() (err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	defer s.guard(&err)
	if s.closed || s.state == "closed" {
		return nil
	}
	n, err := s.now()
	if err != nil {
		return err
	}
	if n-s.lastReceived >= 3_000_000 {
		return errors.New("peer heartbeat timeout")
	}
	expired := false
	gs := []types.Grant{}
	for _, g := range s.grants {
		if g.Expires <= n {
			expired = true
		} else {
			gs = append(gs, g)
		}
	}
	if expired {
		if err = s.grant(gs); err != nil {
			return err
		}
	}
	for _, r := range s.ledger.data.Actions {
		if accepted(r) && r.Outcome == nil {
			a, _ := actionFrom(r.Envelope)
			if a["deadline"].(uint64) <= n {
				s.stopNative()
				if _, err = s.complete(s.contextFor(r), "expired", nil, "deadline_exceeded"); err != nil {
					return err
				}
			}
		}
	}
	s.prune()
	if s.state == "ready" && n-s.lastHealth >= 1_000_000 {
		s.lastHealth = n
		return s.health()
	}
	return nil
}
func privateFrame(raw []byte) bool {
	v, e := types.Decode(wire, raw)
	if e != nil {
		return true
	}
	m := v.(map[string]any)
	return m["type"] == "read_reply" || (m["type"] == "event" && m["body"].(map[string]any)["kind"] != "action.outcome")
}
func authorityFrame(raw []byte) bool {
	v, e := types.Decode(wire, raw)
	if e != nil {
		return true
	}
	m := v.(map[string]any)
	return m["type"] == "grants" || m["type"] == "capabilities"
}
func (s *Surface) purge(authority bool) {
	q := s.queue
	s.queue = nil
	for _, r := range q {
		if !privateFrame(r.raw) && (!authority || !authorityFrame(r.raw)) {
			s.queue = append(s.queue, r)
		}
	}
}

// Kill applies the physical stop before waiting on the control lock or storage.
// It remains latched across restarts and reconnects. Wire messages cannot reset it.
func (s *Surface) Kill(reason string) (err error) {
	s.killed.Store(true)
	s.stopNative()
	s.mu.Lock()
	defer s.mu.Unlock()
	defer s.guard(&err)
	if s.closed {
		return errors.New("closed")
	}
	s.ledger.data.Killed = true
	s.grants = nil
	if s.ledger.data.Epoch == math.MaxUint64 {
		return errors.New("epoch exhausted")
	}
	s.ledger.data.Epoch++
	s.purge(true)
	if err = s.cancel("cancelled", "cancelled"); err != nil {
		return err
	}
	if err = s.save(); err != nil {
		return err
	}
	q := s.queue
	s.queue = nil
	if err = s.emit("kill", map[string]any{"reason": reason, "correlation_id": ID()}); err != nil {
		return err
	}
	if err = s.health(); err != nil {
		return err
	}
	s.queue = append(s.queue, q...)
	if len(s.queue) > 128 {
		return errors.New("resource_exhausted")
	}
	return nil
}
func (s *Surface) Reset() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed || s.state != "closed" {
		return errors.New("reset requires disconnected local operator")
	}
	s.ledger.data.Killed = false
	if err := s.save(); err != nil {
		return err
	}
	s.killed.Store(false)
	return nil
}
func (s *Surface) Disconnect() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed {
		return nil
	}
	return s.abort()
}
func (s *Surface) Close() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed {
		return nil
	}
	err := s.abort()
	s.closed = true
	s.ledger.close()
	return err
}
func (s *Surface) Remove(name string) (err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	defer s.guard(&err)
	if _, ok := s.operations[name]; !ok {
		return fmt.Errorf("unknown operation %s", name)
	}
	gs := make([]types.Grant, 0, len(s.grants))
	for _, g := range s.grants {
		ops := []string{}
		for _, n := range g.Operations {
			if n != name {
				ops = append(ops, n)
			}
		}
		g.Operations = ops
		gs = append(gs, g)
	}
	if err = s.grant(gs); err != nil {
		return err
	}
	delete(s.operations, name)
	s.revision++
	return s.capabilities()
}

// Ensure the SDK uses the same native codec value model as generated records.
var _ codec.Valuer = types.Action{}
