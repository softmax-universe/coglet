"""Independent Git SHA-256 tree oracle for fixture generation/validation.

Record/code/manifest hashes must not use this function (ADR-0035).
"""
import hashlib


def object_id(kind, body):
    return hashlib.sha256(kind + b' ' + str(len(body)).encode() + b'\0' + body).digest()


def snapshot_hash(files):
    directories = {'': []}
    for path, contents in files.items():
        parent, name = path.rsplit('/', 1)
        directories.setdefault(parent, []).append((name.encode(), False, object_id(b'blob', bytes.fromhex(contents))))
        while parent:
            directories.setdefault(parent, [])
            parent = parent.rsplit('/', 1)[0]
    for path in sorted(directories, reverse=True):
        entries = sorted(directories[path], key=lambda e: e[0] + (b'/' if e[1] else b''))
        body = b''.join((b'40000 ' if tree else b'100644 ') + name + b'\0' + oid
                        for name, tree, oid in entries)
        oid = object_id(b'tree', body)
        if not path:
            return oid.hex()
        parent, name = path.rsplit('/', 1)
        directories[parent].append((name.encode(), True, oid))
