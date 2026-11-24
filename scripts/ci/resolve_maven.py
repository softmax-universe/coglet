#!/usr/bin/env python3
"""Resolve the CI Maven closure offline; only preparation may contact Central.

Normal mode may also restore the pinned, hash-checked seed from a GitHub release
(never Central) when the mirror and Actions cache both miss. Merge-queue runs
cannot read caches saved by pull_request runs, so without this a PR that changes
the cache key (pom.xml, this file) could never pass the queue. Proof mode never
seeds: it must show the mirror bundle alone is complete.
"""
import argparse
import os
from pathlib import Path
import subprocess
import time
import tempfile
import tarfile

GOAL = 'org.apache.maven.plugins:maven-dependency-plugin:3.11.0:go-offline'


def seed_maven(repository, deadline):
    # Bootstrap PREPARATION only from the original reviewed outage-proof closure.
    # This is a partial cache seed, never a claim that old assets have new keys.
    from dependency_bundle import unpack
    release = 'dependency-candidate-9b5f54198673ff089905d82db874ace4a27bd101'
    name = 'java-maven-2f4be3935fea788d6b2cd5c2a0a64e1f422ae78b903cf18def207d417c770cfc.tar.gz'
    expected = '5957675c303a908498be1b2c9e88475f691806dc043c7046e087fe87ef52a061'
    remaining = deadline - time.monotonic()
    if remaining <= 0 or not os.environ.get('GH_TOKEN'):
        return False
    try:
        with tempfile.TemporaryDirectory(prefix='maven-seed-') as directory:
            subprocess.run(['gh', 'release', 'download', release, '--repo', 'softmax-universe/coglet',
                            '--pattern', name, '--dir', directory], check=True, timeout=remaining)
            unpack(Path(directory) / name,
                   {'repository': Path(repository) if repository else Path.home() / '.m2/repository'}, expected)
        print(f'Preparation seeded from {release}/{name}; sha256={expected}; full tests and fresh capture still required', flush=True)
        return True
    except (OSError, ValueError, tarfile.TarError, subprocess.SubprocessError) as error:
        print(f'Maven preparation seed unavailable: {error}', flush=True)
        return False


def resolve(prepare=False, repository=None, seed=True):
    command = ['mvn', '--batch-mode', '--no-transfer-progress', '--offline',
               '-f', 'sdk/surface-java/pom.xml', GOAL]
    if repository is not None:
        command.append(f'-Dmaven.repo.local={repository}')
    deadline = time.monotonic() + 165

    def attempt(offline, limit=None):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return 1
        args = command if offline else [part for part in command if part != '--offline']
        try:
            return subprocess.run(args, timeout=min(remaining, limit or remaining)).returncode
        except subprocess.TimeoutExpired:
            return 1

    status = attempt(True, 30 if prepare else None)
    if status and (prepare or seed) and seed_maven(repository, deadline):
        status = attempt(True)
    if status and prepare:
        # Only a read-only preparation job may fill a genuinely cold closure.
        status = attempt(False)
        if status == 0:
            status = attempt(True)
    return status


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prepare', action='store_true')
    parser.add_argument('--proof', action='store_true',
                        help='mirror proof: never restore the pinned seed')
    parser.add_argument('--repository', type=Path)
    args = parser.parse_args()
    try:
        status = resolve(args.prepare, args.repository, seed=not args.proof)
    except subprocess.TimeoutExpired:
        status = 1
    if status:
        print('Maven closure unavailable offline; run dependency preparation to populate the mirror/cache.', flush=True)
    elif not args.prepare and 'GITHUB_ENV' in os.environ:
        # Preparation must also discover dynamically loaded test providers before
        # capture (go-offline alone does not include Surefire's JUnit provider).
        with open(os.environ['GITHUB_ENV'], 'a') as stream:
            stream.write('MAVEN_ARGS=--offline\n')
    return status


if __name__ == '__main__':
    raise SystemExit(main())
