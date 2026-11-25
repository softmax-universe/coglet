#!/usr/bin/env python3
"""Select PR changes that exercise the Swift/native runtime boundary.

Rust implementation changes still get apple-check on every PR and the full Mac
build on main pushes and merge groups. Keep this list aligned with the C header
and xcframework producer, not every transitive Rust implementation dependency.
"""
import sys

PREFIXES = (
    "apps/softmax-mac/",
    "surfaces/face-mac/",
    "sdk/surface-swift/",
    "crates/coglet-runtime/include/",
    "coglets/speech-echo/",
)
FILES = {
    "crates/coglet-runtime/src/ffi.rs",
    "crates/coglet-runtime/src/surface.rs",
    "crates/coglet-runtime/src/lib.rs",
    "crates/coglet-runtime/Cargo.toml",
    "crates/coglet-runtime/cbindgen.toml",
    "scripts/build-runtime-xcframework.sh",
    "scripts/ci/softmax_mac_paths.py",
    "scripts/ci/test_softmax_mac_paths.py",
    ".github/workflows/softmax-mac.yml",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".cargo/config.toml",
}


def needs_mac(paths):
    return any(path in FILES or path.startswith(PREFIXES) for path in paths)


if __name__ == "__main__":
    # NUL-delimited Git names preserve spaces/newlines and include deletions.
    paths = sys.stdin.buffer.read().decode("utf-8", errors="surrogateescape").split("\0")
    print(str(needs_mac(paths)).lower())
