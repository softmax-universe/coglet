"""Fail-closed cross-spec lexical guards (independent of CDDL codegen).

Patterns use Python/jsonschema's actual search behavior. For every pattern,
positive witnesses come from schema/fixture strings and seeded rstr generation;
no positive witness is a coverage error, not a pass. Each positive is retested
with LF appended. CDDL occurrence tokens are scanned outside strings/comments.
"""
import argparse
from collections import Counter
import json
from pathlib import Path
import random
import re

import rstr


def entries(value, path=()):
    if isinstance(value, dict):
        for key, child in value.items():
            yield path + (key,), child
            yield from entries(child, path + (key,))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield path + (index,), child
            yield from entries(child, path + (index,))


def cddl_code(source):
    """Mask literals/comments, preserving offsets and line numbers."""
    chars = list(source)
    quote = None
    comment = escaped = False
    for index, char in enumerate(source):
        if comment:
            if char == '\n':
                comment = False
            else:
                chars[index] = ' '
        elif quote:
            if char != '\n':
                chars[index] = ' '
            if escaped:
                escaped = False
            elif char == '\\':
                escaped = True
            elif char == quote:
                quote = None
        elif char == ';':
            comment = True
            chars[index] = ' '
        elif char in ('"', "'"):
            quote = char
            chars[index] = ' '
    return ''.join(chars)


def lint(root):
    root = Path(root)
    spec = root / 'spec'
    documents = [(path, json.loads(path.read_text())) for path in sorted(spec.rglob('*.json'))]
    candidates = sorted({value for _, document in documents for _, value in entries(document)
                         if isinstance(value, str)})
    findings = []
    for path, document in documents:
        for location, pattern in entries(document):
            if not location or location[-1] != 'pattern':
                continue
            ref = {'file': str(path.relative_to(root)), 'location': list(location)}
            if not isinstance(pattern, str):
                findings.append(dict(ref, kind='pattern-tool-error', error='pattern must be text'))
                continue
            try:
                expression = re.compile(pattern)
                generator = rstr.Rstr(random.Random(0))
                examples = set(candidates)
                for _ in range(32):
                    try:
                        examples.add(generator.xeger(pattern))
                    except (ValueError, KeyError, NotImplementedError):
                        # rstr bounds repeats at 100 and cannot generate every
                        # valid regex. Actual fixture witnesses remain usable;
                        # absent matching witnesses still fail closed below.
                        break
                positives = sorted((example for example in examples if expression.search(example)),
                                   key=lambda value: (len(value), value))
                if not positives:
                    findings.append(dict(ref, kind='pattern-no-positive-witness', pattern=pattern))
                    continue
                for example in positives:
                    # Compare actual jsonschema search with whole-string
                    # matching. LF consumed by the grammar (e.g. a filename
                    # character class) is valid, not an anchor bypass.
                    appended = example + '\n'
                    if expression.search(appended) and not expression.fullmatch(appended):
                        findings.append(dict(ref, kind='pattern-trailing-newline',
                                             pattern=pattern, example=example))
                        break
            except (ValueError, KeyError, NotImplementedError, re.error) as error:
                findings.append(dict(ref, kind='pattern-tool-error', pattern=pattern, error=str(error)))
    paths = set(spec.rglob('*.cddl'))
    # Generated source is also checked; CDDL-looking strings may live inside
    # the emitted target language, so do not mask those target string literals.
    generated_suffixes = {'.py', '.rs', '.go', '.swift', '.java', '.json', '.cddl'}
    for base in ['crates', 'sdk', 'codegen']:
        paths.update(p for p in (root / base).rglob('*')
                     if p.is_file() and p.suffix in generated_suffixes
                     and ('generated' in p.name.lower() or p.name == 'schema.json')
                     and '__pycache__' not in p.parts)
    bounds = re.compile(r'(?<![\w])(?:[0-9]+)?\*\s+[0-9]+|(?<![\w])[0-9]+\s+\*[0-9]*')
    for path in sorted(paths):
        source = path.read_text()
        code = cddl_code(source) if path.suffix == '.cddl' else source
        for match in bounds.finditer(code):
            findings.append({'file': str(path.relative_to(root)), 'kind': 'spaced-cddl-bound',
                             'line': source.count('\n', 0, match.start()) + 1,
                             'token': match.group()})
    return findings


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    findings = lint(args.root)
    report = {'findings': findings, 'hits_per_file': dict(sorted(Counter(f['file'] for f in findings).items())),
              'total': len(findings)}
    if args.output:
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
    raise SystemExit(bool(findings))
