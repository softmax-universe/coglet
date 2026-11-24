//go:build linux

package surface

import (
	"context"
	"encoding/binary"
	"errors"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"sync/atomic"
	"syscall"
	"testing"
	"time"
)

func TestFramingFragmentationCoalescingAndFatalLengths(t *testing.T) {
	raw := []byte{0xa0}
	f, e := Frame(raw)
	must(t, e)
	var p Frames
	var got [][]byte
	for _, b := range append(f, f...) {
		v, e := p.Feed([]byte{b})
		must(t, e)
		got = append(got, v...)
	}
	must(t, p.EOF())
	if !reflect.DeepEqual(got, [][]byte{raw, raw}) {
		t.Fatal(got)
	}
	for _, n := range []uint32{0, MaxFrame + 1} {
		var p Frames
		header := make([]byte, 4)
		binary.BigEndian.PutUint32(header, n)
		if _, e := p.Feed(header); e == nil {
			t.Fatal("bad length accepted")
		}
	}
	p = Frames{}
	_, e = p.Feed(f[:len(f)-1])
	must(t, e)
	if p.EOF() != io.ErrUnexpectedEOF {
		t.Fatal("partial frame accepted")
	}
}
func readFrame(t *testing.T, c net.Conn) map[string]any {
	t.Helper()
	must(t, c.SetReadDeadline(time.Now().Add(3*time.Second)))
	h := make([]byte, 4)
	_, e := io.ReadFull(c, h)
	must(t, e)
	n := binary.BigEndian.Uint32(h)
	if n == 0 || n > MaxFrame {
		t.Fatal("bad output length")
	}
	raw := make([]byte, n)
	_, e = io.ReadFull(c, raw)
	must(t, e)
	m, e := decodeMessage(raw)
	must(t, e)
	return m
}
func sendFragments(t *testing.T, c net.Conn, raw []byte) {
	t.Helper()
	fr, e := Frame(raw)
	must(t, e)
	must(t, c.SetWriteDeadline(time.Now().Add(3*time.Second)))
	for _, b := range fr {
		_, e = c.Write([]byte{b})
		must(t, e)
	}
}
func socketDir(t *testing.T) string {
	t.Helper() // long task TMPDIR must not exceed sockaddr_un.sun_path
	dir, e := os.MkdirTemp("", "cg-")
	must(t, e)
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	return dir
}
func TestAuthenticatedUnixRealSurfaceAndFatalDisconnect(t *testing.T) {
	h := newHost(t, false)
	must(t, h.s.Disconnect())
	l, e := ListenUnix(filepath.Join(socketDir(t), "s"), uint32(os.Getuid()), int32(os.Getpid()))
	must(t, e)
	defer l.Close()
	done := make(chan error, 1)
	go func() {
		c, e := l.Accept()
		if e != nil {
			done <- e
			return
		}
		defer c.Close()
		done <- ServeUnix(context.Background(), h.s, c, ServeOptions{HelloID: testID, Configured: func() error { return h.s.Grant(h.grants()) }})
	}()
	c, e := DialUnix(l.path, uint32(os.Getuid()), int32(os.Getpid()))
	must(t, e)
	defer c.Close()
	if readFrame(t, c)["type"] != "hello" {
		t.Fatal("no hello")
	}
	reply, e := message("hello_reply", map[string]any{"selection": map[string]any{"version": "0.1"}, "correlation_id": testID})
	must(t, e)
	sendFragments(t, c, reply)
	if readFrame(t, c)["type"] != "capabilities" || readFrame(t, c)["type"] != "grants" {
		t.Fatal("bad handshake")
	}
	sendFragments(t, c, action(t, testID, 2, 10000, args()))
	if body(readFrame(t, c))["accepted"] != true || body(readFrame(t, c))["kind"] != "action.outcome" {
		t.Fatal("no native dispatch")
	}
	_, e = c.Write([]byte{0, 0, 0, 0})
	must(t, e)
	select {
	case e := <-done:
		if e == nil {
			t.Fatal("malformed frame not fatal")
		}
	case <-time.After(3 * time.Second):
		t.Fatal("no fatal disconnect")
	}
	if h.s.Ready() || h.effects != 1 {
		t.Fatal("surface did not disconnect/execute")
	}
}
func TestUnixRejectsWrongUIDAndUnsafePaths(t *testing.T) {
	dir := socketDir(t)
	l, e := ListenUnix(filepath.Join(dir, "s"), uint32(os.Getuid()+1), 0)
	must(t, e)
	defer l.Close()
	done := make(chan error, 1)
	go func() {
		c, e := l.Accept()
		if c != nil {
			_ = c.Close()
		}
		done <- e
	}()
	c, e := net.Dial("unix", l.path)
	must(t, e)
	defer c.Close()
	if e = <-done; e == nil {
		t.Fatal("wrong UID allowed")
	}
	if _, e = ListenUnix(l.path, uint32(os.Getuid()), 0); e == nil {
		t.Fatal("preexisting path replaced")
	}
	must(t, os.Chmod(dir, 0755))
	if _, e = ListenUnix(filepath.Join(dir, "other"), uint32(os.Getuid()), 0); e == nil {
		t.Fatal("unsafe directory accepted")
	}
	must(t, os.Chmod(dir, 0700))
}
func TestSurfaceConformance(t *testing.T) {
	dir := t.TempDir()
	binary := filepath.Join(dir, "surface")
	cmd := exec.Command("go", "build", "-o", binary, "./cmd/conformance")
	if out, e := cmd.CombinedOutput(); e != nil {
		t.Fatalf("build adapter: %s %v", out, e)
	}
	cmd = exec.Command("python3", "check_conformance.py", "--binary", binary, "--junit", filepath.Join(dir, "surface.xml"))
	if out, e := cmd.CombinedOutput(); e != nil {
		t.Fatalf("shared surface cases: %s %v", out, e)
	} else {
		t.Log(string(out))
	}
}

func socketPair(t *testing.T) (int, net.Conn) {
	t.Helper()
	fds, e := syscall.Socketpair(syscall.AF_UNIX, syscall.SOCK_STREAM|syscall.SOCK_CLOEXEC, 0)
	must(t, e)
	f := os.NewFile(uintptr(fds[1]), "peer")
	c, e := net.FileConn(f)
	must(t, e)
	must(t, f.Close())
	t.Cleanup(func() { _ = c.Close(); _ = syscall.Close(fds[0]) })
	return fds[0], c
}
func readAck(t *testing.T, c net.Conn) uint64 {
	t.Helper()
	must(t, c.SetReadDeadline(time.Now().Add(3*time.Second)))
	var raw [8]byte
	_, e := io.ReadFull(c, raw[:])
	must(t, e)
	return binary.BigEndian.Uint64(raw[:])
}

// The ADR-0021 fixture clock: time moves only by explicit advances, and every advance
// and frame is acknowledged with the logical time only after its output is written.
func TestFixtureClockAcknowledgesAfterOutputAndRejectsRetrograde(t *testing.T) {
	h := newHost(t, false)
	must(t, h.s.Disconnect())
	data, peer := socketPair(t)
	ctl, clockPeer := socketPair(t)
	var logical atomic.Uint64
	done := make(chan error, 1)
	go func() {
		done <- ServeFD(context.Background(), h.s, data, data, ServeOptions{HelloID: testID,
			Configured: func() error { return h.s.Grant(h.grants()) },
			Control:    ctl,
			Advance: func(m uint64) error {
				if m < logical.Load() {
					return errors.New("retrograde")
				}
				logical.Store(m)
				return nil
			},
			Now: logical.Load})
	}()
	if readFrame(t, peer)["type"] != "hello" {
		t.Fatal("no hello")
	}
	reply, e := message("hello_reply", map[string]any{"selection": map[string]any{"version": "0.1"}, "correlation_id": testID})
	must(t, e)
	sendFragments(t, peer, reply)
	if got := readAck(t, clockPeer); got != 0 {
		t.Fatalf("frame ack %d, want logical 0", got)
	}
	// The ack means the replies are already written: they must not need further time.
	if readFrame(t, peer)["type"] != "capabilities" || readFrame(t, peer)["type"] != "grants" {
		t.Fatal("bad handshake")
	}
	for _, micros := range []uint64{100_000, 100_000, 110_000} {
		var raw [8]byte
		binary.BigEndian.PutUint64(raw[:], micros)
		_, e = clockPeer.Write(raw[:])
		must(t, e)
		if got := readAck(t, clockPeer); got != micros {
			t.Fatalf("advance ack %d, want %d", got, micros)
		}
	}
	var raw [8]byte
	binary.BigEndian.PutUint64(raw[:], 109_999)
	_, e = clockPeer.Write(raw[:])
	must(t, e)
	select {
	case e = <-done:
		if e == nil || !strings.Contains(e.Error(), "retrograde") {
			t.Fatalf("retrograde advance accepted: %v", e)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("retrograde advance did not stop the server")
	}
}
