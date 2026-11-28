"""Strict receipt boundary for the opt-in hosted proof.

This checks observed metadata only. It does not authenticate a server or prove
commit/input bindings. The caller must obtain these notes through its trusted
Unforged client, verify the object graph and enforce export authority separately.
Synthetic tests of this module are never hosted qualification.
"""
import hashlib
import json
import re


class Refused(ValueError):
    """Missing, ambiguous, or contradictory execution evidence."""


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise Refused('duplicate receipt field')
        result[key] = value
    return result


def validate_receipt(raw, *, commit, image, memo_hit, fresh):
    """Validate exactly one invocation receipt, refusing multi-entry ambiguity.

    Raw refs/notes/receipts may contain several observations; an integration
    must bind a unique invocation, not silently select the final line. This
    strict initial boundary rejects multiple lines rather than guessing.
    `fresh=False` validates a warm-control HIT, never fresh gate acceptance.
    """
    if type(memo_hit) is not bool or type(fresh) is not bool:
        raise Refused('boolean cache/freshness controls required')
    if not isinstance(commit, str) or re.fullmatch('[0-9a-f]{64}', commit) is None:
        raise Refused('full SHA-256 result commit required')
    if (not isinstance(image, str)
            or image.startswith(('unforged@', 'docker://unforged@'))
            or re.fullmatch(r'[^\s@]+@sha256:[0-9a-f]{64}', image) is None):
        raise Refused('resolved immutable worker image required')
    if not isinstance(raw, str) or len(raw.encode()) > 16384:
        raise Refused('bounded raw receipt required')
    lines = [line for line in raw.splitlines() if line.strip()]
    if len(lines) != 1:
        raise Refused('missing or ambiguous invocation receipt')
    try:
        value = json.loads(lines[0], object_pairs_hook=_unique_object)
    except (ValueError, RecursionError) as error:
        raise Refused('invalid receipt JSON') from error
    if (not isinstance(value, dict)
            or set(value) != {'runner', 'worker_image', 'executed', 'cache', 'commit'}):
        raise Refused('closed execution receipt required')
    if (value['runner'] != 'caos' or value['worker_image'] != image
            or value['commit'] != commit or type(value['executed']) is not bool):
        raise Refused('remote execution identity/binding mismatch')
    if fresh:
        if memo_hit or value['executed'] is not True or value['cache'] != 'none':
            raise Refused('fresh gate requires remote execution without cache')
    elif not memo_hit or value['executed'] is not False or value['cache'] not in ('local', 'caos'):
        raise Refused('warm control must identify a cache hit without execution')
    return value


def validate_invocation(*, raw_commit, raw_receipt, commit, output_tree,
                        program_commit, input_commit, resolver, image,
                        memo_hit, fresh):
    """Bind trusted-client receipt observations to the retained request graph.

    The caller must supply actual raw Git object bytes with replacements disabled,
    not `git show` pretty output or candidate-constructed graph descriptors. It
    must independently verify the parent commits' trees, resolver and full input
    envelope against retained pins. This guard never authenticates CAOS by itself.
    """
    for oid in (commit, output_tree, program_commit, input_commit, resolver):
        if not isinstance(oid, str) or re.fullmatch('[0-9a-f]{64}', oid) is None:
            raise Refused('full SHA-256 graph identities required')
    if type(raw_commit) is not bytes or len(raw_commit) > 1048576:
        raise Refused('bounded raw commit object required')
    calculated = hashlib.sha256(b'commit ' + str(len(raw_commit)).encode()
                                + b'\0' + raw_commit).hexdigest()
    if calculated != commit:
        raise Refused('commit object identity mismatch')
    try:
        header, message = raw_commit.decode('utf-8').split('\n\n', 1)
    except (ValueError, UnicodeError) as error:
        raise Refused('invalid raw commit encoding') from error
    headers = header.splitlines()
    trees = [line[5:] for line in headers if line.startswith('tree ')]
    parents = [line[7:] for line in headers if line.startswith('parent ')]
    if trees != [output_tree] or parents != [program_commit, input_commit]:
        raise Refused('returned tree or ordered input/program graph mismatch')
    resolver_lines = [line for line in message.splitlines()
                      if line.startswith('Resolver-Commit:')]
    memo_lines = [line for line in message.splitlines() if line.startswith('Memo-Hit:')]
    if (resolver_lines != ['Resolver-Commit: ' + resolver]
            or type(memo_hit) is not bool
            or memo_lines != ['Memo-Hit: ' + str(memo_hit).lower()]):
        raise Refused('resolver or cache provenance mismatch')
    return validate_receipt(raw_receipt, commit=commit, image=image,
                            memo_hit=memo_hit, fresh=fresh)
