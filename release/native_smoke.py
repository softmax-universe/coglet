#!/usr/bin/env python3
"""Fail-closed ARMv7 native release ELF and QEMU smoke (not EO1 qualification)."""
import argparse
from pathlib import Path
import re
import subprocess


def inspect(path):
    raw = path.read_bytes()
    if (len(raw) < 52 or raw[:6] != b'\x7fELF\x01\x01'
            or int.from_bytes(raw[18:20], 'little') != 40
            or not int.from_bytes(raw[36:40], 'little') & 0x400):
        raise ValueError(f'{path}: expected ARM ELF32 little-endian hard-float')
    versions = subprocess.check_output(['readelf', '--version-info', str(path)], text=True)
    needed = {tuple(map(int, v.split('.'))) for v in re.findall(r'\bGLIBC_([0-9.]+)', versions)}
    if not needed or max(needed) > (2, 27):
        raise ValueError(f'{path}: requires unsupported glibc versions {sorted(needed)}')
    program = subprocess.check_output(['readelf', '--program-headers', str(path)], text=True)
    if '/lib/ld-linux-armhf.so.3' not in program:
        raise ValueError(f'{path}: unexpected dynamic interpreter')
    dynamic = subprocess.check_output(['readelf', '--dynamic', str(path)], text=True)
    libraries = set(re.findall(r'\(NEEDED\).*?\[([^]]+)\]', dynamic))
    # Zig also records the exact interpreter above as a NEEDED library.
    if not libraries or libraries - {'ld-linux-armhf.so.3', 'libc.so.6', 'libm.so.6', 'libdl.so.2', 'libpthread.so.0', 'librt.so.1', 'libgcc_s.so.1'}:
        raise ValueError(f'{path}: unqualified shared libraries {sorted(libraries)}')
    print(f'{path.name}: max GLIBC_{".".join(map(str, max(needed)))}; NEEDED={sorted(libraries)}', flush=True)


def smoke(bins, runner):
    for name in ('cogletd', 'coglet-runtime-git-validator'):
        inspect(bins / name)
    def run(name, *args):
        return subprocess.run([*runner, str(bins / name), *args], capture_output=True, text=True, timeout=30)
    version = run('cogletd', '--version')
    if version.returncode or not version.stdout.startswith('cogletd '):
        raise ValueError(f'version smoke failed: {version}')
    features = run('cogletd', '--features')
    if features.returncode or features.stdout.strip() != 'engine=wamr device=true validator=coglet-runtime-git-validator':
        raise ValueError(f'native feature smoke failed: {features}')
    helper = run('coglet-runtime-git-validator')
    if helper.returncode != 20 or 'helper arguments' not in helper.stdout:
        raise ValueError(f'validator argument-refusal smoke failed: {helper}')
    print(version.stdout + features.stdout + helper.stdout, end='')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bins', type=Path, required=True)
    parser.add_argument('--sysroot', default='/usr/arm-linux-gnueabihf')
    args = parser.parse_args()
    smoke(args.bins.resolve(), ['qemu-arm', '-L', args.sysroot])
