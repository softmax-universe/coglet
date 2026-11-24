//go:build linux

package surface

import (
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"syscall"
	"time"
)

// Frames incrementally parses fragmented/coalesced u32be frames. Invalid lengths
// are rejected before payload allocation. EOF within a frame is always fatal.
type Frames struct {
	buffer []byte
	size   uint32
}

func (f *Frames) Feed(data []byte) (out [][]byte, err error) {
	for len(data) > 0 {
		need := 4
		if f.size != 0 {
			need = int(f.size)
		}
		take := min(need-len(f.buffer), len(data))
		f.buffer = append(f.buffer, data[:take]...)
		data = data[take:]
		if len(f.buffer) < need {
			continue
		}
		if f.size == 0 {
			f.size = binary.BigEndian.Uint32(f.buffer)
			f.buffer = nil
			if f.size == 0 || f.size > MaxFrame {
				return nil, errors.New("invalid frame length")
			}
		} else {
			out = append(out, f.buffer)
			f.buffer = nil
			f.size = 0
		}
	}
	return
}
func (f *Frames) EOF() error {
	if len(f.buffer) != 0 || f.size != 0 {
		return io.ErrUnexpectedEOF
	}
	return nil
}
func Frame(raw []byte) ([]byte, error) {
	if len(raw) == 0 || len(raw) > MaxFrame {
		return nil, errors.New("invalid frame length")
	}
	b := make([]byte, 4+len(raw))
	binary.BigEndian.PutUint32(b, uint32(len(raw)))
	copy(b[4:], raw)
	return b, nil
}

// UnixListener binds only an absent path in an owned private directory and
// authenticates runtime UID plus optional supervisor PID using SO_PEERCRED.
// Same-UID hostile code requires an OS sandbox, as specified by the protocol.
type UnixListener struct {
	listener *net.UnixListener
	path     string
	inode    uint64
	uid      uint32
	pid      int32
}

func ListenUnix(path string, runtimeUID uint32, runtimePID int32) (*UnixListener, error) {
	path, err := filepath.Abs(path)
	if err != nil {
		return nil, err
	}
	if len(path) > 107 {
		return nil, errors.New("Unix socket path too long")
	}
	if err = privateDir(filepath.Dir(path)); err != nil {
		return nil, err
	}
	if _, err = os.Lstat(path); !os.IsNotExist(err) {
		return nil, errors.New("socket path already exists or inaccessible")
	}
	l, err := net.ListenUnix("unix", &net.UnixAddr{Name: path, Net: "unix"})
	if err != nil {
		return nil, err
	}
	l.SetUnlinkOnClose(false)
	fail := func(e error) (*UnixListener, error) { _ = l.Close(); _ = os.Remove(path); return nil, e }
	if err = os.Chmod(path, 0600); err != nil {
		return fail(err)
	}
	st, err := os.Lstat(path)
	if err != nil {
		return fail(err)
	}
	return &UnixListener{l, path, st.Sys().(*syscall.Stat_t).Ino, runtimeUID, runtimePID}, nil
}
func (l *UnixListener) Accept() (*net.UnixConn, error) {
	c, err := l.listener.AcceptUnix()
	if err != nil {
		return nil, err
	}
	raw, err := c.SyscallConn()
	if err == nil {
		var auth error
		err = raw.Control(func(fd uintptr) {
			u, e := syscall.GetsockoptUcred(int(fd), syscall.SOL_SOCKET, syscall.SO_PEERCRED)
			if e != nil {
				auth = e
				return
			}
			if u.Uid != l.uid || (l.pid != 0 && u.Pid != l.pid) {
				auth = errors.New("unauthenticated runtime peer")
			}
		})
		if err == nil {
			err = auth
		}
	}
	if err != nil {
		_ = c.Close()
		return nil, err
	}
	return c, nil
}

// DialUnix connects when the supervisor provisions a runtime-owned socket.
// It enforces the same private-path and OS-credential binding as ListenUnix;
// transport connection is not enrollment and carries no bearer credentials.
func DialUnix(path string, runtimeUID uint32, runtimePID int32) (*net.UnixConn, error) {
	if err := privateDir(filepath.Dir(path)); err != nil {
		return nil, err
	}
	st, err := os.Lstat(path)
	if err != nil {
		return nil, err
	}
	if st.Mode()&os.ModeSocket == 0 || st.Mode().Perm() != 0600 || st.Sys().(*syscall.Stat_t).Uid != runtimeUID {
		return nil, errors.New("unsafe runtime socket")
	}
	c, err := net.DialUnix("unix", nil, &net.UnixAddr{Name: path, Net: "unix"})
	if err != nil {
		return nil, err
	}
	raw, err := c.SyscallConn()
	if err == nil {
		var auth error
		err = raw.Control(func(fd uintptr) {
			u, e := syscall.GetsockoptUcred(int(fd), syscall.SOL_SOCKET, syscall.SO_PEERCRED)
			if e != nil {
				auth = e
			} else if u.Uid != runtimeUID || (runtimePID != 0 && u.Pid != runtimePID) {
				auth = errors.New("unauthenticated runtime peer")
			}
		})
		if err == nil {
			err = auth
		}
	}
	if err != nil {
		_ = c.Close()
		return nil, err
	}
	return c, nil
}

func (l *UnixListener) Close() error {
	err := l.listener.Close()
	if st, e := os.Lstat(l.path); e == nil && st.Sys().(*syscall.Stat_t).Ino == l.inode {
		err = errors.Join(err, os.Remove(l.path))
	}
	return err
}

// ServeOptions callbacks execute on the serialized control owner, must be
// bounded and must never stand in for a native off-switch. Configured installs
// trusted effective grants after hello_reply. Ready is test-adapter readiness,
// emitted after durable initialization, immediately before the initial hello.
type ServeOptions struct {
	HelloID    string
	Configured func() error
	Poll       func() error
	Ready      func()
	// Control is a test-only fixture clock descriptor (ADR-0021); never set it on a
	// production connection. Zero (stdin) means none. Each 8-byte big-endian value read
	// is a logical microsecond advance, passed to Advance. After a frame or advance has
	// been dispatched, ticked and its output fully written, the logical time Now returns
	// is echoed back as 8 big-endian bytes. Advance errors are fatal.
	Control int
	Advance func(micros uint64) error
	Now     func() uint64
}

// ServeUnix serves one authenticated socket. Listener remains reusable for a
// fresh negotiation with higher grant epoch after disconnect.
func ServeUnix(ctx context.Context, s *Surface, conn *net.UnixConn, opts ServeOptions) error {
	f, err := conn.File()
	if err != nil {
		return err
	}
	defer f.Close()
	return ServeFD(ctx, s, int(f.Fd()), int(f.Fd()), opts)
}

type outputFrame struct {
	raw                []byte
	generation         uint64
	private, authority bool
}

const fdWordBits = strconv.IntSize

func fdset(set *syscall.FdSet, fd int) { set.Bits[fd/fdWordBits] |= 1 << uint(fd%fdWordBits) }
func isset(set *syscall.FdSet, fd int) bool {
	return set.Bits[fd/fdWordBits]&(1<<uint(fd%fdWordBits)) != 0
}

// ServeFD runs an authenticated Linux descriptor pair; stdio is only for the
// conformance adapter. Output is bounded, partial writes retain revocation
// metadata, and revoked partial private frames force disconnect.
func ServeFD(ctx context.Context, s *Surface, in, out int, opts ServeOptions) (err error) {
	if in < 0 || out < 0 || in >= 1024 || out >= 1024 {
		return errors.New("descriptor exceeds select capacity")
	}
	defer func() { err = errors.Join(err, s.Disconnect()) }()
	old := map[int]uintptr{}
	for _, fd := range []int{in, out} {
		if _, ok := old[fd]; ok {
			continue
		}
		flags, _, e := syscall.Syscall(syscall.SYS_FCNTL, uintptr(fd), syscall.F_GETFL, 0)
		if e != 0 {
			return e
		}
		old[fd] = flags
		if e := syscall.SetNonblock(fd, true); e != nil {
			return e
		}
	}
	defer func() {
		for fd, flags := range old {
			_, _, _ = syscall.Syscall(syscall.SYS_FCNTL, uintptr(fd), syscall.F_SETFL, flags)
		}
	}()
	hello, err := s.Open(opts.HelloID)
	if err != nil {
		return err
	}
	if opts.Ready != nil {
		opts.Ready()
	}
	pending := []outputFrame{}
	offset := 0
	add := func(raw []byte, generation uint64) error {
		if len(pending) >= 128 {
			return errors.New("resource_exhausted: transport queue")
		}
		f, e := Frame(raw)
		if e != nil {
			return e
		}
		pending = append(pending, outputFrame{f, generation, privateFrame(raw), authorityFrame(raw)})
		return nil
	}
	if err = add(hello, s.generation.Load()); err != nil {
		return err
	}
	var parser Frames
	buf := make([]byte, 65536)
	ready := false
	blocked := time.Time{}
	control := opts.Control > 0 && opts.Advance != nil && opts.Now != nil
	if control && opts.Control >= 1024 {
		return errors.New("descriptor exceeds select capacity")
	}
	var acks []uint64 // logical times awaiting acknowledgement, one per input
	var advance [8]byte
	have := 0
	for {
		if err = ctx.Err(); err != nil {
			return err
		}
		if opts.Poll != nil {
			if err = opts.Poll(); err != nil {
				return err
			}
		}
		if err = s.Tick(); err != nil {
			return err
		}
		gen, killed := s.generation.Load(), s.killed.Load()
		kept := pending[:0]
		for i, f := range pending {
			revoked := (f.private && (f.generation != gen || killed)) || (killed && f.authority)
			if revoked {
				if i == 0 && offset > 0 {
					return errors.New("revoked partial private/authority frame")
				}
				continue
			}
			kept = append(kept, f)
		}
		pending = kept
		for _, frame := range s.drainFrames() {
			if err = add(frame.raw, frame.generation); err != nil {
				return err
			}
		}
		// Keep kill/stopped-health before other wholly unwritten frames. Never
		// interrupt a partial frame; the reflex was already applied independently.
		start := 0
		if offset > 0 {
			start = 1
		}
		for rank := 0; rank < 2; rank++ {
			for i := start; i < len(pending); i++ {
				raw := pending[i].raw[4:]
				v, e := typesMessage(raw)
				if e != nil {
					return e
				}
				priority := v == "kill" && rank == 0 || v == "stopped" && rank == 1
				if priority {
					f := pending[i]
					copy(pending[start+1:i+1], pending[start:i])
					pending[start] = f
					start++
					break
				}
			}
		}
		// Acknowledge only after the input's tick and all resulting output are written.
		for control && len(pending) == 0 && len(acks) > 0 {
			var raw [8]byte
			binary.BigEndian.PutUint64(raw[:], acks[0])
			for sent := 0; sent < len(raw); {
				n, e := syscall.Write(opts.Control, raw[sent:])
				if e == syscall.EINTR {
					continue
				}
				if e != nil {
					return e
				}
				sent += n
			}
			acks = acks[1:]
		}
		if len(pending) > 0 {
			if blocked.IsZero() {
				blocked = time.Now()
			}
			if time.Since(blocked) >= 3*time.Second {
				return errors.New("blocked peer output")
			}
		} else {
			blocked = time.Time{}
		}
		var r, w syscall.FdSet
		fdset(&r, in)
		if control {
			fdset(&r, opts.Control)
		}
		if len(pending) > 0 {
			fdset(&w, out)
		}
		tv := syscall.Timeval{Usec: 10000}
		_, e := syscall.Select(max(in, out, opts.Control)+1, &r, &w, nil, &tv)
		if e == syscall.EINTR {
			continue
		}
		if e != nil {
			return e
		}
		if isset(&w, out) && len(pending) > 0 {
			// Synchronize the final generation check + actual delivery with native kill.
			s.effect.Lock()
			f := pending[0]
			revoked := (f.private && (f.generation != s.generation.Load() || s.killed.Load())) || (s.killed.Load() && f.authority)
			if revoked {
				s.effect.Unlock()
				if offset > 0 {
					return errors.New("revoked partial frame")
				}
				continue
			}
			n, e := syscall.Write(out, f.raw[offset:])
			s.effect.Unlock()
			if e != nil && e != syscall.EAGAIN && e != syscall.EINTR {
				return e
			}
			if n > 0 {
				offset += n
				blocked = time.Now()
				if offset == len(f.raw) {
					pending = pending[1:]
					offset = 0
				}
			}
		}
		if control && isset(&r, opts.Control) {
			n, e := syscall.Read(opts.Control, advance[have:])
			if n == 0 && e == nil {
				return errors.New("fixture clock closed")
			}
			if e != nil && e != syscall.EAGAIN && e != syscall.EINTR {
				return e
			}
			if n > 0 {
				have += n
			}
			if have == len(advance) {
				have = 0
				if e = opts.Advance(binary.BigEndian.Uint64(advance[:])); e != nil {
					return e
				}
				acks = append(acks, opts.Now())
			}
		}
		if isset(&r, in) {
			n, e := syscall.Read(in, buf)
			if e == syscall.EAGAIN || e == syscall.EINTR {
				continue
			}
			if e != nil {
				return e
			}
			if n == 0 {
				return parser.EOF()
			}
			frames, e := parser.Feed(buf[:n])
			if e != nil {
				return e
			}
			for _, raw := range frames {
				if e = s.Receive(raw); e != nil {
					return e
				}
				if control {
					acks = append(acks, opts.Now())
				}
				if !ready && s.Ready() {
					ready = true
					if opts.Configured != nil {
						if e = opts.Configured(); e != nil {
							return e
						}
					}
				}
			}
		}
	}
}
func typesMessage(raw []byte) (string, error) {
	v, e := decodeMessage(raw)
	if e != nil {
		return "", e
	}
	kind := v["type"].(string)
	if kind == "health" && v["body"].(map[string]any)["state"] == "stopped" {
		return "stopped", nil
	}
	return kind, nil
}
func (l *UnixListener) String() string { return fmt.Sprintf("coglet surface socket %s", l.path) }
