//go:build linux

package main

import (
	"errors"
	"os"
	"syscall"
)

// fixtureControl connects the harness-owned test-control socket named by
// COGLET_FIXTURE_CLOCK_SOCKET (ADR-0021) and returns its blocking descriptor, or 0 when
// the harness did not opt this adapter in. It is never a surface protocol message.
func fixtureControl() (int, error) {
	path := os.Getenv("COGLET_FIXTURE_CLOCK_SOCKET")
	if path == "" {
		return 0, nil
	}
	fd, err := syscall.Socket(syscall.AF_UNIX, syscall.SOCK_STREAM|syscall.SOCK_CLOEXEC, 0)
	if err != nil {
		return 0, err
	}
	if fd < 3 || fd >= 1024 {
		_ = syscall.Close(fd)
		return 0, errors.New("fixture clock descriptor out of range")
	}
	for {
		err = syscall.Connect(fd, &syscall.SockaddrUnix{Name: path})
		if err != syscall.EINTR {
			break
		}
	}
	if err != nil {
		_ = syscall.Close(fd)
		return 0, err
	}
	return fd, nil
}
