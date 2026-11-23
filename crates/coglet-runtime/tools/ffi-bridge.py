#!/usr/bin/env python3
"""Raw framing bridge to the ACTUAL embedded C ABI, not a protocol simulator."""
import ctypes as C
from pathlib import Path
import struct
import sys

def load(library, config):
    lib = C.CDLL(str(library))
    lib.coglet_runtime_configure.argtypes = [C.c_void_p, C.c_size_t]
    lib.coglet_runtime_configure.restype = C.c_int32
    lib.coglet_surface_open.argtypes = [C.c_uint32, C.POINTER(C.c_void_p)]
    lib.coglet_surface_open.restype = C.c_int32
    lib.coglet_surface_send.argtypes = [C.c_void_p, C.c_void_p, C.c_size_t]
    lib.coglet_surface_send.restype = C.c_int32
    lib.coglet_surface_recv.argtypes = [C.c_void_p, C.c_void_p, C.c_size_t, C.POINTER(C.c_size_t)]
    lib.coglet_surface_recv.restype = C.c_int32
    lib.coglet_surface_close.argtypes = [C.c_void_p]
    lib.coglet_surface_close.restype = None
    raw = Path(config).read_bytes()
    assert lib.coglet_runtime_configure(raw, len(raw)) == 0
    return lib

def exact(n):
    result = bytearray()
    while len(result) < n:
        part = sys.stdin.buffer.read(n-len(result))
        if not part:
            raise EOFError
        result.extend(part)
    return bytes(result)

if __name__ == "__main__":
    lib = load(sys.argv[1], sys.argv[2])
    link = C.c_void_p()
    assert lib.coglet_surface_open(1, C.byref(link)) == 0
    try:
        while True:
            n = struct.unpack(">I", exact(4))[0]
            if not 0 < n <= 1_048_576:
                raise ValueError("invalid frame length")
            raw = exact(n)
            status = lib.coglet_surface_send(link, raw, n)
            if status != 0:
                raise RuntimeError(f"send failed: {status}")
            while True:
                needed = C.c_size_t()
                status = lib.coglet_surface_recv(link, None, 0, C.byref(needed))
                if status == -2:
                    sys.exit(0)
                if status == 1:
                    break
                assert status == 2
                buf = C.create_string_buffer(needed.value)
                assert lib.coglet_surface_recv(link, buf, len(buf), C.byref(needed)) == 0
                sys.stdout.buffer.write(struct.pack(">I", needed.value) + buf.raw[:needed.value])
                sys.stdout.buffer.flush()
    except EOFError:
        pass
    finally:
        lib.coglet_surface_close(link)
