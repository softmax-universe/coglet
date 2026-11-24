#!/usr/bin/env python3
"""Proof-only registry fence, before the existing CI commands execute."""
import os
from pathlib import Path
import shutil
import subprocess

HOSTS = ('repo.maven.apache.org', 'repo1.maven.org', 'crates.io',
         'index.crates.io', 'static.crates.io', 'static.rust-lang.org')


def main():
    entries = ''.join(f'0.0.0.0 {host}\n::1 {host}\n' for host in HOSTS)
    subprocess.run(['sudo', 'tee', '-a', '/etc/hosts'], input=entries, text=True, check=True)
    for host in HOSTS:
        for family in ('-4', '-6'):
            result = subprocess.run(['curl', '--noproxy', '*', family, '--fail',
                                     '--connect-timeout', '5', '--max-time', '10', f'https://{host}/'],
                                    stdout=subprocess.DEVNULL)
            if result.returncode == 0:
                raise RuntimeError(f'registry fence failed: {host} {family}')
            print(f'DENIED {host} {family}: curl exit {result.returncode}', flush=True)
    # Reach temporary POM subprocesses too, not only top-level mvn invocations.
    original = shutil.which('mvn')
    if not original:
        raise RuntimeError('runner Maven installation missing')
    bindir = Path(os.environ['RUNNER_TEMP']) / 'offline-bin'
    bindir.mkdir(exist_ok=True)
    wrapper = bindir / 'mvn'
    wrapper.write_text(f'#!/bin/sh\nexec "{original}" --offline "$@"\n')
    wrapper.chmod(0o755)
    with open(os.environ['GITHUB_PATH'], 'a') as stream:
        stream.write(f'{bindir}\n')
    with open(os.environ['GITHUB_ENV'], 'a') as stream:
        stream.write('CARGO_NET_OFFLINE=true\nMAVEN_ARGS=--offline\n')
    print('Cargo offline and global Maven offline wrapper enabled')


if __name__ == '__main__':
    main()
