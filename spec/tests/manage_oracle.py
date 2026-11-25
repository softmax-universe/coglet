"""ADR-0031 spec-only filesystem/receipt oracle; not a runtime implementation."""
import hashlib
import json
import re
from test_common import encode, decode

MAX_PATCH = 64 * 1024 * 1024
MAX_TAIL = 65536
REMOVAL = '2027-02-25T00:00:00Z'


def normalize(method, now):
    # The normative spellings and deadline live in the protocol alias table.
    legacy = 'de' + 'bug'
    if now < REMOVAL and method in {legacy + '.' + n for n in ('read', 'write', 'pause', 'step')}:
        return 'manage.' + method.split('.')[1]
    return method


def tree(files):
    """ABI snapshot hash, restricted to absolute /code paths; no new hash format."""
    return hashlib.sha256(encode({'/code/' + p: h for p, h in files.items()})).hexdigest()


def signing(patch, hub, coglet):
    fields = {k: v for k, v in patch.items() if k != 'signature'}
    return b'coglet/hub/0:manage.patch\n' + encode(dict(fields, hub=hub, coglet=coglet))


def code_path(path):
    return (isinstance(path, str) and re.fullmatch(r'[a-zA-Z0-9_.-]{1,128}(/[a-zA-Z0-9_.-]{1,128}){0,31}', path)
            and all(part not in {'.', '..'} for part in path.split('/')))


class Rejected(ValueError):
    def __init__(self, code, reason):
        self.code, self.reason = code, reason
        super().__init__(reason)


def size_check(size):
    if not 1 <= size <= MAX_PATCH:
        raise Rejected('resource_exhausted', 'patch_size')


def apply(files, generation, patch, installed=True):
    """Signature/authority checked by caller. Pure candidate; never mutates input."""
    size_check(len(encode(patch)))
    paths = [c['path'] for c in patch['files']]
    if (not 1 <= len(paths) <= 4096 or any(not code_path(p) for p in paths)
            or paths != sorted(set(paths), key=lambda p: p.encode('utf-8'))):
        raise Rejected('invalid_argument', 'path')
    if ('base' in patch) != installed or (installed and patch['base'] != tree(files)):
        raise Rejected('conflict', 'stale_base')
    if patch['generation'] <= generation:
        raise Rejected('conflict', 'generation')
    result = dict(files)
    for change in patch['files']:
        path = change['path']
        if 'bytes' in change:
            result[path] = change['bytes']
        else:
            result.pop(path, None)
    if any(a != b and b.startswith(a + '/') for a in result for b in result):
        raise Rejected('invalid_argument', 'file_directory_collision')
    if tree(result) != patch['revision']:
        raise Rejected('invalid_argument', 'revision')
    module = bytes.fromhex(result.get('bin/step.wasm', ''))
    if not module.startswith(b'\x00asm\x01\x00\x00\x00'):
        raise Rejected('invalid_argument', 'executable')
    # Full guest ABI/module validation is the real receiver's separate obligation.
    return result, patch['generation']


def log_bytes(path, file_offset, content):
    # Same canonical JSONL convention as ABI inbox; logs may contain arbitrary bytes.
    if (not isinstance(path, str) or not path.startswith('/logs/')
            or any(part in {'', '.', '..'} for part in path[1:].split('/'))
            or any(ord(c) < 32 or ord(c) == 127 or c == '\\' for c in path)):
        raise Rejected('invalid_argument', 'log_path')
    # Python permits lone surrogates; the wire/ABI requires actual UTF-8.
    try:
        path.encode('utf-8')
    except UnicodeEncodeError as e:
        raise Rejected('invalid_argument', 'log_path') from e
    assert content
    return (json.dumps({'path': path, 'file_offset': file_offset, 'hex': content.hex()},
                       sort_keys=True, separators=(',', ':'), ensure_ascii=False) + '\n').encode()


def tail(stream, offset, limit=MAX_TAIL):
    if not 0 <= offset <= len(stream):
        raise Rejected('invalid_argument', 'offset')
    chunk = stream[offset:offset + min(limit, MAX_TAIL)]
    return chunk, offset + len(chunk)
