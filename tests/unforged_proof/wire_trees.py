"""Verify raw SHA-1 CAOS trees and derive exact SHA-256 ABI trees, no projection."""
import hashlib
from standard_step import Refused, path_parts

def object_id(kind, raw, algorithm='sha1'):
    return hashlib.new(algorithm, kind.encode()+b' '+str(len(raw)).encode()+b'\0'+raw).hexdigest()

def file_tree(files, algorithm='sha1'):
    root = {}
    for path, raw in files.items():
        node = root
        parts = path_parts(path)
        for part in parts[:-1]: node = node.setdefault(part,{})
        node[parts[-1]] = raw
    def tree(node):
        rows = []
        for name,value in sorted(node.items(),key=lambda kv:(kv[0]+('/' if isinstance(kv[1],dict) else '')).encode()):
            mode = '40000' if isinstance(value,dict) else '100644'
            oid = tree(value) if isinstance(value,dict) else object_id('blob',value,algorithm)
            rows.append(mode.encode()+b' '+name.encode()+b'\0'+bytes.fromhex(oid))
        return object_id('tree',b''.join(rows),algorithm)
    return tree(root)

def verify_output(rows, root):
    objects = {}
    for row in rows:
        if row.get('phase') != 'abi-object': continue
        if set(row) != {'phase','oid','kind','hex'} or row['kind'] not in ('tree','blob'):
            raise Refused('invalid raw object envelope')
        try: raw = bytes.fromhex(row['hex'])
        except (ValueError,TypeError): raise Refused('invalid raw object bytes') from None
        if object_id(row['kind'],raw) != row['oid']:
            raise Refused('raw object hash mismatch')
        value = (row['kind'],raw)
        if row['oid'] in objects and objects[row['oid']] != value:
            raise Refused('conflicting raw object')
        objects[row['oid']] = value
    seen = set()
    def convert(oid,kind,depth):
        if depth > 32 or oid not in objects or objects[oid][0] != kind:
            raise Refused('missing/wrong/deep object')
        seen.add(oid)
        raw = objects[oid][1]
        if kind == 'blob': return object_id('blob',raw,'sha256')
        offset, names, rows, keys = 0, set(), [], []
        while offset < len(raw):
            try:
                stop = raw.index(b'\0',offset)
                mode,name = raw[offset:stop].split(b' ',1)
                text = name.decode('utf-8')
                if len(path_parts(text)) != 1 or text in names or mode not in (b'40000',b'100644'):
                    raise Refused('nonregular/duplicate tree entry')
                child = raw[stop+1:stop+21]
                if len(child) != 20: raise Refused('truncated tree')
            except (ValueError,UnicodeError): raise Refused('invalid tree encoding') from None
            names.add(text)
            keys.append(name+(b'/' if mode == b'40000' else b''))
            converted = convert(child.hex(),'tree' if mode == b'40000' else 'blob',depth+1)
            rows.append(mode+b' '+name+b'\0'+bytes.fromhex(converted))
            offset = stop+21
        if keys != sorted(keys): raise Refused('noncanonical tree order')
        return object_id('tree',b''.join(rows),'sha256')
    result = convert(root,'tree',0)
    if seen != set(objects): raise Refused('unreachable extra output objects')
    return result
