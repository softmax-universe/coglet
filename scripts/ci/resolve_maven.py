#!/usr/bin/env python3
"""Resolve the CI Maven closure offline; only preparation may contact Central."""
import argparse
import os
from pathlib import Path
import subprocess
import time

GOAL = 'org.apache.maven.plugins:maven-dependency-plugin:3.11.0:go-offline'


def resolve(prepare=False, repository=None):
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
    if status and prepare:
        # Only a read-only preparation job may fill a genuinely cold closure.
        status = attempt(False)
        if status == 0:
            status = attempt(True)
    return status


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prepare', action='store_true')
    parser.add_argument('--repository', type=Path)
    args = parser.parse_args()
    try:
        status = resolve(args.prepare, args.repository)
    except subprocess.TimeoutExpired:
        status = 1
    if status:
        print('Maven closure unavailable offline; run dependency preparation to populate the mirror/cache.', flush=True)
    elif 'GITHUB_ENV' in os.environ:
        with open(os.environ['GITHUB_ENV'], 'a') as stream:
            stream.write('MAVEN_ARGS=--offline\n')
    return status


if __name__ == '__main__':
    raise SystemExit(main())
