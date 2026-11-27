#!/usr/bin/env python3
"""Fail-closed native artifact check for the app-owned DEBUG wire-proof seam."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile

MARKERS = (
    b'AppSurfaceWireProof', b'WireProofExecutor', b'wireProofShortcutPresses',
    b'WireProofCredentialFixture', b'CredentialFixtureStore', b'CredentialFixtureLink',
    b'installWireProofHubFixture', b'--sdk-wire-proof-directory',
    b'--sdk-wire-proof-credentials', b'credential-witness',
)


def validate_projection(symbols, strings, *, release):
    hits = [m.decode('ascii') for m in MARKERS if m in symbols or m in strings]
    if release and hits:
        raise ValueError('forbidden wire-proof markers: ' + ', '.join(hits))
    return hits


# All 32/64-bit Mach-O and fat byte orders. Do not infer images from filenames.
MACHO = {bytes.fromhex(x) for x in (
    'feedface', 'cefaedfe', 'feedfacf', 'cffaedfe',
    'cafebabe', 'bebafeca', 'cafebabf', 'bfbafeca',
)}
FAT = {bytes.fromhex(x) for x in ('cafebabe', 'bebafeca', 'cafebabf', 'bfbafeca')}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run_tool(args):
    result = subprocess.run(args, capture_output=True, check=False, timeout=120)
    if result.returncode or result.stderr:
        raise ValueError(f'native tool failed: {args[0]} exit={result.returncode} stderr={result.stderr!r}')
    return result.stdout


def audit_bundle(app, output, *, release):
    app = Path(app).resolve(strict=True)
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
    main = app / 'Contents/MacOS/Softmax'
    images = []
    for entry in sorted(app.rglob('*')):
        target = entry.resolve(strict=True)
        if not target.is_relative_to(app):
            raise ValueError(f'escaping bundle symlink: {entry}')
        if entry.is_symlink() or not entry.is_file():
            continue  # canonical real target is visited once, including framework Versions/A
        with entry.open('rb') as handle:
            magic = handle.read(4)
        if magic in MACHO:
            images.append((entry, magic))
    if main not in [p for p, _ in images]:
        raise ValueError('missing Mach-O primary executable')
    rows = []
    for image, magic in images:
        original_hash = digest(image)
        arch_text = run_tool(['/usr/bin/lipo', '-archs', str(image)]).decode('ascii').strip()
        archs = arch_text.split()
        if not archs or len(archs) != len(set(archs)) or any(not re.fullmatch(r'[A-Za-z0-9_]+', a) for a in archs):
            raise ValueError('invalid architecture inventory')
        if magic not in FAT and len(archs) != 1:
            raise ValueError('thin image has multiple architectures')
        for arch in archs:
            with tempfile.TemporaryDirectory(prefix='softmax-release-audit-') as tmp:
                thin = Path(tmp) / 'image'
                if magic in FAT:
                    run_tool(['/usr/bin/lipo', '-thin', arch, str(image), '-output', str(thin)])
                else:
                    thin = image
                symbols = run_tool(['/usr/bin/nm', str(thin)])
                strings = run_tool(['/usr/bin/strings', '-a', str(thin)])
                if not symbols.strip() or not strings.strip():
                    raise ValueError('empty native inspection output')
                ordinal = len(rows)
                symbol_path = f'{ordinal:04d}-{arch}.symbols.txt'
                string_path = f'{ordinal:04d}-{arch}.strings.txt'
                (output / symbol_path).write_bytes(symbols)
                (output / string_path).write_bytes(strings)
                row = {'image': str(image.relative_to(app)), 'arch': arch,
                       'image_sha256': original_hash, 'slice_sha256': digest(thin),
                       'symbols': symbol_path, 'symbols_sha256': hashlib.sha256(symbols).hexdigest(),
                       'strings': string_path, 'strings_sha256': hashlib.sha256(strings).hexdigest(),
                       'hits': validate_projection(symbols, strings, release=release)}
                rows.append(row)
        if digest(image) != original_hash:
            raise ValueError('image changed during inspection')
    return rows


def validate_debug(rows):
    app_archs = {r['arch'] for r in rows if r['image'] == 'Contents/MacOS/Softmax'}
    if not app_archs:
        raise ValueError('missing Debug app architecture')
    for arch in sorted(app_archs):
        hits = {hit for row in rows if row['arch'] == arch for hit in row['hits']}
        missing = [m.decode('ascii') for m in MARKERS if m.decode('ascii') not in hits]
        if missing:
            raise ValueError(f'missing Debug markers in {arch}: {missing}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--release', type=Path, required=True)
    parser.add_argument('--debug', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    # A stale success report must never survive a failed invocation.
    args.output.mkdir(parents=True, exist_ok=True)
    report = args.output / 'report.json'
    report.unlink(missing_ok=True)
    debug = audit_bundle(args.debug, args.output / 'debug', release=False)
    validate_debug(debug)
    release = audit_bundle(args.release, args.output / 'release', release=True)
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=Path(__file__).parent, text=True).strip()
    report.write_text(json.dumps({'version': 1, 'head_sha': head, 'auditor_sha256': digest(Path(__file__)),
                                  'markers': [m.decode('ascii') for m in MARKERS],
                                  'debug': debug, 'release': release, 'passed': True}, indent=2) + '\n')
    print(f'PASS Release exclusion: {len(release)} image/slices; Debug positive: {len(debug)} image/slices')


if __name__ == '__main__':
    main()
