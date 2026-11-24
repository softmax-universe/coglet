#!/usr/bin/env python3
"""Flaky-test quarantine for required checks.

A test listed in ci/quarantine.toml is skipped by the required checks (rust,
conformance) and run instead by the non-required `stability` workflow, until its
owner card makes it deterministic and removes the entry. One flaky test must not
eject merge-queue entries for everyone; it must also not be forgotten, so every
entry names an owner card and a start date, and entries older than
MAX_AGE_DAYS fail the stability workflow.

  quarantine.py check [--fail-expired]   validate the file (and its age)
  quarantine.py rust-args [--only]       libtest arguments (one per line): skip, or select only, quarantined tests
"""
import argparse
import datetime
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
DEFAULT = ROOT / 'ci' / 'quarantine.toml'
KINDS = ('conformance', 'rust')
FIELDS = {'id', 'card', 'since', 'reason'}
MAX_AGE_DAYS = 14
CARD = re.compile(r't_[0-9a-f]{16}')
# conformance: "<implementation>.<suite>/<case>", exactly as conformance/run prints it.
# rust: the libtest path inside its test binary, e.g. "runaway_deadline_then_fresh_success"
# or "tests::name"; matched with --exact, so it never quarantines more than one name.
IDS = {
    'conformance': re.compile(r'[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)+/[A-Za-z0-9_.-]+'),
    'rust': re.compile(r'[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*'),
}


def load(path=DEFAULT):
    path = Path(path)
    data = tomllib.loads(path.read_text()) if path.exists() else {}
    unknown = set(data) - set(KINDS)
    if unknown:
        raise ValueError(f'unknown quarantine sections: {sorted(unknown)}')
    result = {}
    for kind in KINDS:
        entries = data.get(kind, [])
        if not isinstance(entries, list):
            raise ValueError(f'[{kind}] must be an array of tables ([[{kind}]])')
        seen = set()
        for entry in entries:
            if not isinstance(entry, dict) or set(entry) != FIELDS:
                raise ValueError(f'[[{kind}]] entries need exactly {sorted(FIELDS)}: {entry!r}')
            ident, card, since, reason = entry['id'], entry['card'], entry['since'], entry['reason']
            if not isinstance(ident, str) or not IDS[kind].fullmatch(ident):
                raise ValueError(f'[[{kind}]] invalid id: {ident!r}')
            if not isinstance(card, str) or not CARD.fullmatch(card):
                raise ValueError(f'{ident}: card must be a board card id like t_0123456789abcdef')
            if type(since) is not datetime.date:
                raise ValueError(f'{ident}: since must be a TOML date like 2026-11-24')
            if not isinstance(reason, str) or not reason.strip():
                raise ValueError(f'{ident}: reason must say what fails and where it was seen')
            if ident in seen:
                raise ValueError(f'[[{kind}]] duplicate id: {ident}')
            seen.add(ident)
        result[kind] = entries
    return result


def expired(quarantine, today):
    return [(kind, entry) for kind in KINDS for entry in quarantine[kind]
            if (today - entry['since']).days > MAX_AGE_DAYS]


def rust_args(quarantine, only=False):
    """Arguments after `cargo test ... --`. Empty means: nothing quarantined."""
    ids = [entry['id'] for entry in quarantine['rust']]
    if not ids:
        return []
    if only:
        return ['--exact', *ids]
    return ['--exact', *[arg for ident in ids for arg in ('--skip', ident)]]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--file', type=Path, default=DEFAULT)
    commands = parser.add_subparsers(dest='command', required=True)
    check = commands.add_parser('check')
    check.add_argument('--fail-expired', action='store_true')
    check.add_argument('--today', type=datetime.date.fromisoformat, default=datetime.date.today())
    args_cmd = commands.add_parser('rust-args')
    args_cmd.add_argument('--only', action='store_true')
    args = parser.parse_args(argv)
    try:
        quarantine = load(args.file)
    except (ValueError, tomllib.TOMLDecodeError) as error:
        print(f'{args.file}: {error}', file=sys.stderr)
        return 1
    if args.command == 'rust-args':
        print('\n'.join(rust_args(quarantine, args.only)))  # one argument per line
        return 0
    total = sum(len(quarantine[kind]) for kind in KINDS)
    for kind in KINDS:
        for entry in quarantine[kind]:
            age = (args.today - entry['since']).days
            print(f'QUARANTINED {kind} {entry["id"]} card={entry["card"]} age={age}d: {entry["reason"]}')
    stale = expired(quarantine, args.today)
    for kind, entry in stale:
        print(f'EXPIRED {kind} {entry["id"]}: quarantined over {MAX_AGE_DAYS} days; '
              f'card {entry["card"]} must fix it or remove the test', file=sys.stderr)
    print(f'{total} quarantined test(s); {len(stale)} expired')
    return 1 if args.fail_expired and stale else 0


if __name__ == '__main__':
    raise SystemExit(main())
