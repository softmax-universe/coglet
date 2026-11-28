"""Strict STANDARD-job input and whole ABI tree helpers. No hosted PASS alone."""
import hashlib
import json

class Refused(ValueError):
    pass

def path_parts(path):
    if not isinstance(path, str) or not path or '\0' in path or '\\' in path:
        raise Refused('invalid snapshot path')
    parts = path.split('/')
    if any(p in ('', '.', '..', '.git') for p in parts):
        raise Refused('unmappable snapshot path')
    return parts

def snapshot_tree(files):
    if isinstance(files, list):
        entries = {}
        for row in files:
            if set(row) != {'path','mode','hex'} or row['mode'] != '100644' or row['path'] in entries:
                raise Refused('nonregular or duplicate returned tree entry')
            try:
                entries[row['path']] = bytes.fromhex(row['hex'])
            except (ValueError, TypeError):
                raise Refused('invalid tree bytes') from None
        files = entries
    root = {}
    for path, raw in files.items():
        parts = path_parts(path)
        node = root
        for part in parts[:-1]:
            if part in node and not isinstance(node[part], dict):
                raise Refused('file/directory collision')
            node = node.setdefault(part, {})
        if parts[-1] in node or not isinstance(raw, bytes):
            raise Refused('duplicate or colliding path')
        node[parts[-1]] = raw
    def obj(kind, raw):
        return hashlib.sha256(kind+b' '+str(len(raw)).encode()+b'\0'+raw).digest()
    def tree(node):
        rows = []
        for name, value in sorted(node.items(), key=lambda kv: (kv[0]+('/' if isinstance(kv[1],dict) else '')).encode()):
            mode = b'40000' if isinstance(value, dict) else b'100644'
            oid = tree(value) if isinstance(value, dict) else obj(b'blob', value)
            rows.append(mode+b' '+name.encode()+b'\0'+oid)
        return obj(b'tree', b''.join(rows))
    return tree(root).hex()

def state_files(snapshot, payload):
    files = {}
    for path, value in snapshot.items():
        if not isinstance(path, str) or not path.startswith('/') or path == '/proof-input.json':
            raise Refused('invalid prior snapshot')
        try:
            files[path[1:]] = bytes.fromhex(value)
        except (TypeError, ValueError):
            raise Refused('invalid prior bytes') from None
    snapshot_tree(files)  # validate complete paths before staging any file
    files['proof-input.json'] = json.dumps(payload, separators=(',',':')).encode()
    return files
