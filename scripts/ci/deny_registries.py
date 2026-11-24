#!/usr/bin/env python3
"""Proof-only registry fence, before the existing CI commands execute."""
import os
import subprocess

HOSTS = ('crates.io',
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
    with open(os.environ['GITHUB_ENV'], 'a') as stream:
        stream.write('CARGO_NET_OFFLINE=true\n')
    print('Cargo offline enabled')


if __name__ == '__main__':
    main()
