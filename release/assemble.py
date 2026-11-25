#!/usr/bin/env python3
"""Release assembly. GitHub publication is gated separately in release.yml."""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import re
import subprocess
import tarfile

PLATFORMS = ('linux-x86_64', 'linux-aarch64', 'linux-armv7', 'macos-arm64')


def version(tag):
    number = r'(?:0|[1-9][0-9]*)'
    if not re.fullmatch(rf'v{number}\.{number}\.{number}(?:-rc[1-9][0-9]*)?', tag):
        raise ValueError('expected vMAJOR.MINOR.PATCH or vMAJOR.MINOR.PATCH-rcN (N >= 1)')
    return tag[1:].replace('-rc', 'rc')


def expected(tag):
    py = version(tag)
    return [*(f'coglet-{tag}-{platform}.tar.gz' for platform in PLATFORMS),
            f'coglet-{tag}-linux-armv7-native.tar.gz',
            *(f'coglet-{tag}-{name}.tar.gz' for name in ('rust-source', 'swift-source', 'go-source', 'harness')),
            f'coglet_surface-{py}-py3-none-any.whl', f'coglet_surface-{py}.tar.gz']


def source_bundles(root, out, tag, sha):
    """All sources retain workspace-relative paths; never ship working-tree data."""
    version(tag)
    if not re.fullmatch('[0-9a-f]{40}', sha):
        raise ValueError('source must be a full commit SHA')
    out.mkdir(parents=True, exist_ok=True)
    raw = subprocess.check_output(['git', '-C', str(root), 'archive', '--format=tar',
                                   f'--prefix=coglet-{tag}/', sha])
    # Complete, identical source snapshots intentionally preserve path dependencies
    # and harness adapters. Different names provide stable consumer entry points.
    data = gzip.compress(raw, mtime=0)
    for name in ('rust-source', 'swift-source', 'go-source', 'harness'):
        (out / f'coglet-{tag}-{name}.tar.gz').write_bytes(data)


def binary_bundle(bins, out, tag, platform, native=False):
    version(tag)
    if platform not in PLATFORMS:
        raise ValueError('unsupported platform')
    if native and platform != 'linux-armv7':
        raise ValueError('native release profile requires linux-armv7')
    payloads = {}
    names = ('cogletd', 'coglet-runtime-git-validator') if native else ('cogletd', 'coglet-hub')
    for name in names:
        path = bins / name
        raw = path.read_bytes()
        magic = b'\x7fELF' if platform.startswith('linux-') else b'\xcf\xfa\xed\xfe'
        if raw[:4] != magic or not path.stat().st_mode & 0o111:
            raise ValueError(f'{name} is not an executable for {platform}')
        if platform.startswith('linux-'):
            machine = int.from_bytes(raw[18:20], 'little')
            wanted = {'linux-x86_64': (2, 62), 'linux-aarch64': (2, 183), 'linux-armv7': (1, 40)}[platform]
            if len(raw) < 52 or raw[5] != 1 or (raw[4], machine) != wanted:
                raise ValueError(f'{name} ELF architecture mismatch for {platform}')
        elif len(raw) < 32 or int.from_bytes(raw[4:8], 'little') != 0x0100000C:
            raise ValueError(f'{name} Mach-O architecture mismatch')
        payloads[name] = raw
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode='w') as archive:
        for name, raw in payloads.items():
            entry = tarfile.TarInfo(name)
            entry.size = len(raw)
            entry.mode = 0o755
            archive.addfile(entry, io.BytesIO(raw))
    out.mkdir(parents=True, exist_ok=True)
    suffix = '-native' if native else ''
    (out / f'coglet-{tag}-{platform}{suffix}.tar.gz').write_bytes(gzip.compress(stream.getvalue(), mtime=0))


def seal(out, tag, sha):
    names = set(expected(tag))
    if not re.fullmatch('[0-9a-f]{40}', sha):
        raise ValueError('source must be a full commit SHA')
    actual = {p.name for p in out.iterdir()}
    if actual != names:
        raise ValueError(f'incomplete inventory: missing={sorted(names - actual)}, extra={sorted(actual - names)}')
    assets = []
    for name in sorted(names):
        path = out / name
        if path.is_symlink() or not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f'invalid asset {name}')
        assets.append({'name': name, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(), 'size': path.stat().st_size})
    manifest = {'tag': tag, 'commit': sha, 'assets': assets,
                'qualification': 'Unsuffixed binaries use the reference engine. linux-armv7-native contains WAMR and its lifecycle validator, GNU hard-float glibc 2.27 target; QEMU smoke is not product qualification.',
                'go_tag': f'sdk/surface-go/{tag}', 'swift_tag': tag}
    path = out / 'release.json'
    path.write_text(json.dumps(manifest, indent=2) + '\n')
    sums = [f"{asset['sha256']}  {asset['name']}\n" for asset in assets]
    sums.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  release.json\n")
    (out / 'SHA256SUMS').write_text(''.join(sums))


def python_stage(root, dest, tag, sha):
    py = version(tag)
    if not re.fullmatch('[0-9a-f]{40}', sha):
        raise ValueError('source must be a full commit SHA')
    if dest.exists():
        raise ValueError('Python stage must not already exist')
    raw = subprocess.check_output(['git', '-C', str(root), 'archive', '--format=tar', sha, 'sdk/surface-python'])
    dest.mkdir(parents=True)
    with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
        archive.extractall(dest, filter='data')
    manifest = dest / 'sdk/surface-python/pyproject.toml'
    text, count = re.subn(r'(?m)^version = "[^"]+"$', f'version = "{py}"', manifest.read_text())
    if count != 1:
        raise ValueError('expected exactly one Python package version')
    manifest.write_text(text)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['sources', 'binaries', 'seal', 'python-stage'])
    parser.add_argument('--tag', required=True)
    parser.add_argument('--sha', default='')
    parser.add_argument('--root', type=Path, default=Path.cwd())
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--bins', type=Path)
    parser.add_argument('--platform', choices=PLATFORMS)
    parser.add_argument('--native', action='store_true', help='separate ARMv7 WAMR daemon + validator asset')
    args = parser.parse_args()
    try:
        version(args.tag)
        if args.operation == 'sources':
            source_bundles(args.root, args.out, args.tag, args.sha)
        elif args.operation == 'binaries':
            if args.bins is None or args.platform is None:
                parser.error('binaries requires --bins and --platform')
            binary_bundle(args.bins, args.out, args.tag, args.platform, native=args.native)
        elif args.operation == 'python-stage':
            python_stage(args.root, args.out, args.tag, args.sha)
        else:
            seal(args.out, args.tag, args.sha)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'{error}\n')


if __name__ == '__main__':
    main()

