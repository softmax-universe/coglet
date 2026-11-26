package surface

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"syscall"
)

const maxLedger = 64 << 20

type record struct {
	Envelope []byte
	Accept   []byte
	Outcome  []byte
	End      uint64
}
type durable struct {
	Identity                    string
	Epoch, Clock, Seq, Snapshot uint64
	Killed                      bool
	Running                     bool
	Actions                     map[string]*record
	// ContractProfile and CatalogDigest are the replay identity pair (ADR-0030
	// 1a). Records without them are exactly surface-0.1 with no catalog.
	ContractProfile string `json:"contract_profile,omitempty"`
	CatalogDigest   string `json:",omitempty"`
	Catalog         []byte `json:",omitempty"`
}
type ledger struct {
	path string
	lock *os.File
	data durable
}
type sealed struct {
	Data   json.RawMessage
	SHA256 string
}

// The stable side lock survives atomic snapshot replacement. All ledger files
// live in an owned 0700 directory. Losing a ledger requires reconciliation and a
// new surface identity; Open never silently recreates one.
func openLedger(path, identity string, create bool, profile, digest string, catalog []byte) (*ledger, error) {
	if err := privateDir(filepath.Dir(path)); err != nil {
		return nil, err
	}
	fd, err := syscall.Open(path+".lock", syscall.O_RDWR|syscall.O_CREAT|syscall.O_NOFOLLOW, 0600)
	if err != nil {
		return nil, err
	}
	l := &ledger{path: path, lock: os.NewFile(uintptr(fd), path+".lock")}
	fail := func(e error) (*ledger, error) { l.close(); return nil, e }
	if err = privateFile(l.lock); err != nil {
		return fail(err)
	}
	if err = syscall.Flock(fd, syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		return fail(fmt.Errorf("ledger already owned: %w", err))
	}
	if create {
		f, e := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
		if e != nil {
			return fail(e)
		}
		if e = f.Close(); e != nil {
			return fail(e)
		}
		l.data = durable{Identity: identity, Actions: map[string]*record{}, ContractProfile: profile, CatalogDigest: digest, Catalog: append([]byte(nil), catalog...)}
		if e = l.save(); e != nil {
			return fail(e)
		}
	} else {
		fd, e := syscall.Open(path, syscall.O_RDONLY|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0)
		if e != nil {
			return fail(e)
		}
		f := os.NewFile(uintptr(fd), path)
		e = privateFile(f)
		var raw []byte
		if e == nil {
			raw, e = io.ReadAll(io.LimitReader(f, maxLedger+1))
		}
		_ = f.Close()
		if e != nil {
			return fail(e)
		}
		if len(raw) > maxLedger {
			return fail(errors.New("ledger too large"))
		}
		var s sealed
		if e = json.Unmarshal(raw, &s); e != nil {
			return fail(e)
		}
		sum := sha256.Sum256(s.Data)
		if hex.EncodeToString(sum[:]) != s.SHA256 {
			return fail(errors.New("corrupt ledger checksum"))
		}
		if e = json.Unmarshal(s.Data, &l.data); e != nil {
			return fail(e)
		}
		if l.data.Identity != identity || l.data.Actions == nil {
			return fail(errors.New("lost ledger or identity mismatch; reconcile and rotate identity"))
		}
		// Refuse before any save: a changed pair is an explicit new ledger, never
		// a reinterpretation of old receipts.
		saved := l.data.ContractProfile
		if saved == "" {
			saved = ProfileSurface01
		}
		if saved != ProfileSurface01 && saved != ProfileSurface02 {
			return fail(errors.New("unknown saved contract profile"))
		}
		if saved != profile || l.data.CatalogDigest != digest || !bytes.Equal(l.data.Catalog, catalog) {
			return fail(errors.New("contract profile/catalog digest mismatch; use a new ledger"))
		}
		l.data.ContractProfile = saved
	}
	return l, nil
}
func privateFile(f *os.File) error {
	st, err := f.Stat()
	if err != nil {
		return err
	}
	if !st.Mode().IsRegular() || st.Mode().Perm() != 0600 || st.Sys().(*syscall.Stat_t).Uid != uint32(os.Getuid()) {
		return errors.New("ledger must be an owned private regular file")
	}
	return nil
}
func privateDir(path string) error {
	abs, err := filepath.Abs(path)
	if err != nil {
		return err
	}
	real, err := filepath.EvalSymlinks(abs)
	if err != nil {
		return err
	}
	st, err := os.Lstat(abs)
	if err != nil {
		return err
	}
	if real != abs || !st.IsDir() || st.Mode().Perm() != 0700 || st.Sys().(*syscall.Stat_t).Uid != uint32(os.Getuid()) {
		return errors.New("state/socket directory must be owned 0700 without symlinks")
	}
	return nil
}
func (l *ledger) save() error {
	data, err := json.Marshal(l.data)
	if err != nil {
		return err
	}
	sum := sha256.Sum256(data)
	raw, err := json.Marshal(sealed{data, hex.EncodeToString(sum[:])})
	if err != nil {
		return err
	}
	if len(raw) > maxLedger {
		return errors.New("resource_exhausted: ledger capacity")
	}
	f, err := os.CreateTemp(filepath.Dir(l.path), ".coglet-ledger-*")
	if err != nil {
		return err
	}
	name := f.Name()
	defer os.Remove(name)
	if _, err = f.Write(raw); err == nil {
		err = f.Sync()
	}
	ce := f.Close()
	if err == nil {
		err = ce
	}
	if err != nil {
		return err
	}
	if err = os.Rename(name, l.path); err != nil {
		return err
	}
	dir, err := os.Open(filepath.Dir(l.path))
	if err != nil {
		return err
	}
	defer dir.Close()
	return dir.Sync()
}
func (l *ledger) close() {
	if l.lock != nil {
		_ = l.lock.Close()
		l.lock = nil
	}
}
