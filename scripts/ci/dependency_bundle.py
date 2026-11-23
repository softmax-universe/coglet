#!/usr/bin/env python3
"""Small dependency-only archive transport. No third-party Python dependencies."""
import argparse
import hashlib
import json
import os
import platform
import subprocess
import tomllib
from pathlib import Path
import shutil
import tarfile
import tempfile


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def pack(roots, archive):
    with tarfile.open(archive, 'w:gz', dereference=True) as stream:
        for name, source in roots.items():
            if source.exists():
                stream.add(source, arcname=name)


def unpack(archive, roots, expected):
    if digest(archive) != expected:
        raise ValueError('archive digest mismatch')
    # Validate/extract everything in quarantine before touching any real cache.
    with tempfile.TemporaryDirectory(prefix='dependency-bundle-') as directory:
        stage = Path(directory)
        with tarfile.open(archive) as stream:
            for member in stream.getmembers():
                parts = Path(member.name).parts
                if not parts or parts[0] not in roots or '..' in parts or Path(member.name).is_absolute():
                    raise ValueError('invalid archive root/path')
                if member.islnk():
                    raise ValueError('hardlinks are not permitted')
                if member.issym():
                    link = stage / member.name
                    resolved = (link.parent / member.linkname).resolve()
                    if not resolved.is_relative_to(stage / parts[0]):
                        raise ValueError('link escapes dependency root')
            stream.extractall(stage, filter='data')
        for name, target in roots.items():
            source = stage / name
            if source.exists():
                target.parent.mkdir(parents=True, exist_ok=True)
                if target.is_symlink():
                    target.unlink()
                elif target.exists():
                    shutil.rmtree(target)
                shutil.move(str(source), str(target))


def input_key(root, job, ecosystem):
    # Include setup/closure logic as well as every nested manifest/lock. Source-only
    # changes reuse bundles; new temporary codegen crates invalidate them.
    patterns = ['.github/workflows/*.yml', '.github/actions/dependency-*/*.yml', 'scripts/ci/*.py', 'codegen/*.py',
                '**/build.rs', 'rust-toolchain.toml']
    patterns += {
        'cargo': ['**/Cargo.toml', '**/Cargo.lock', 'sdk/guest-rust/*.sh'],
        'rustup': [],
        'maven': ['**/pom.xml'],
        'python': ['**/requirements*.txt', '**/uv.lock', '**/pyproject.toml'],
        'gems': ['**/Gemfile', '**/Gemfile.lock'],
    }[ecosystem]
    hashed = hashlib.sha256(f'v1|ubuntu-24.04|x86_64|java21|py312|ruby32|{job}|{ecosystem}'.encode())
    # Hosted image identity pins preinstalled Maven/JDK/bootstrap tool versions.
    # A new image deliberately starts cold instead of selecting an incompatible
    # immutable Maven plugin closure. The daily refresh publishes the new set.
    hashed.update(os.environ.get('ImageVersion', 'local').encode())
    tracked = None
    if (root / '.git').exists():
        tracked = set(subprocess.check_output(['git', 'ls-files', '-z'], cwd=root).decode().split('\0'))
    paths = sorted({path for pattern in patterns for path in root.glob(pattern)
                    if path.is_file() and '.git' not in path.parts and 'target' not in path.parts
                    and (tracked is None or str(path.relative_to(root)) in tracked)})
    for path in paths:
        hashed.update(str(path.relative_to(root)).encode() + b'\0' + path.read_bytes() + b'\0')
    return hashed.hexdigest()


def restore_one(name, roots, download, required=False):
    try:
        with tempfile.TemporaryDirectory(prefix='dependency-download-') as directory:
            stage = Path(directory)
            download(name + '.json', stage)
            manifest = json.loads((stage / (name + '.json')).read_text())
            if manifest['name'] != name or manifest['schema'] != 1:
                raise ValueError('bundle identity mismatch')
            download(name + '.tar.gz', stage)
            unpack(stage / (name + '.tar.gz'), roots, manifest['sha256'])
            print(f'Restored {name}: sha256={manifest["sha256"]} source={manifest["source_sha"]}', flush=True)
            return True
    except (OSError, ValueError, KeyError, TypeError, tarfile.TarError, subprocess.SubprocessError) as error:
        print(f'Bundle {name} unavailable: {error}; upstream fallback', flush=True)
        if required:
            raise RuntimeError(f'required dependency bundle {name} unavailable; publish this input set first') from error
        return False


def install_toolchain_on_miss(channel, restored):
    if not restored:
        # An environment override selects a toolchain, not the file's profile,
        # targets or components. Explicitly install that full closure on misses.
        subprocess.run(['rustup', 'toolchain', 'install', channel, '--profile',
                        'minimal', '--component', 'clippy,rustfmt', '--target',
                        'wasm32-wasip1'], check=True)


def cache_roots(root, channel):
    home = Path.home()
    return {
        'maven': {'repository': home / '.m2/repository'},
        'cargo': {'registry': home / '.cargo/registry', 'git': home / '.cargo/git'},
        'rustup': {'toolchain': home / f'.rustup/toolchains/{channel}-x86_64-unknown-linux-gnu'},
        # Python/Ruby are convenience closures, not yet whole-registry-outage proof.
        'python': {'wheels': home / '.cache/coglet-wheels'},
        'gems': {'gems': root / 'spec/tests/vendor/cache'},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['restore', 'pack'])
    parser.add_argument('--job', choices=['java', 'rust', 'conformance'], required=True)
    parser.add_argument('--release', default='dependency-mirror-v1')
    parser.add_argument('--required', action='store_true')
    args = parser.parse_args()
    root = Path.cwd()
    if platform.machine() != 'x86_64' or platform.system() != 'Linux':
        raise RuntimeError('bundles are Linux x86_64 only')
    channel = tomllib.loads((root / 'rust-toolchain.toml').read_text())['toolchain']['channel']
    (Path.home() / '.cache/coglet-wheels').mkdir(parents=True, exist_ok=True)
    if 'GITHUB_ENV' in os.environ and args.job != 'java':
        with open(os.environ['GITHUB_ENV'], 'a') as stream:
            stream.write(f'RUSTUP_TOOLCHAIN={channel}-x86_64-unknown-linux-gnu\n')
            stream.write(f'PIP_FIND_LINKS={Path.home()}/.cache/coglet-wheels\n')
            stream.write(f'UV_FIND_LINKS={Path.home()}/.cache/coglet-wheels\n')
    roots = cache_roots(root, channel)
    ecosystems = {'java': ['maven'], 'rust': ['cargo', 'rustup', 'python'],
                  'conformance': ['maven', 'cargo', 'rustup', 'python', 'gems']}[args.job]
    def download(name, directory):
        subprocess.run(['gh', 'release', 'download', args.release, '--repo', os.environ['GITHUB_REPOSITORY'],
                        '--pattern', name, '--dir', str(directory)], check=True, timeout=180)
    out = root / 'dependency-bundles'
    for ecosystem in ecosystems:
        name = f'{args.job}-{ecosystem}-{input_key(root, args.job, ecosystem)}'
        if args.operation == 'restore':
            restored = restore_one(name, roots[ecosystem], download,
                                   required=args.required and ecosystem in {'maven', 'cargo', 'rustup'})
            if ecosystem == 'rustup':
                install_toolchain_on_miss(channel, restored)
        else:
            out.mkdir(exist_ok=True)
            archive = out / (name + '.tar.gz')
            pack(roots[ecosystem], archive)
            if archive.stat().st_size >= 2 * 1024**3:
                raise RuntimeError('bundle exceeds release asset 2 GiB limit')
            manifest = {'schema': 1, 'name': name, 'sha256': digest(archive),
                        'source_sha': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
                        'bytes': archive.stat().st_size}
            (out / (name + '.json')).write_text(json.dumps(manifest, indent=2) + '\n')
            print(json.dumps(manifest), flush=True)
            if 'GITHUB_STEP_SUMMARY' in os.environ:
                with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as stream:
                    stream.write(f'- `{name}`: {manifest["bytes"]} bytes, SHA-256 `{manifest["sha256"]}`\n')


if __name__ == '__main__':
    main()
