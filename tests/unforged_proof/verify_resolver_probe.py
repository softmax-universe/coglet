#!/usr/bin/env python3
"""Offline integrity readback of actual resolver-control objects; not fresh auth."""
import argparse
import hashlib
import json
from pathlib import Path
from resolver_controls import verify_delta,verify_sequence
from hosted_receipts import Refused
from guarded_transport import strict_json


def verify(batch,*,source,driver,bridge,image,endpoint,resolver):
    command=strict_json((batch/'command.json').read_bytes())
    pins=command['pins']
    if (command['source']!=source or command['driver_sha256']!=driver or command['bridge_sha256']!=bridge
            or pins['image']!=image or pins['endpoint']!=endpoint or pins['resolver_a']!=resolver):
        raise Refused('independent source/tool/endpoint/image/resolver pin differs')
    trace_raw=(batch/'trace.json').read_bytes();trace=strict_json(trace_raw)
    if hashlib.sha256(trace_raw).hexdigest()!=command['trace_sha256'] or trace['records'][command['step']]['output_hash']!=pins['tree']:
        raise Refused('original native oracle differs')
    checked=verify_sequence((batch/'bridge.jsonl').read_text(),**pins)
    kinds=strict_json((batch/'objects.json').read_bytes())
    objects={}
    for oid,kind in kinds.items():
        raw=(batch/'objects'/oid).read_bytes()
        if hashlib.sha256(kind.encode()+b' '+str(len(raw)).encode()+b'\0'+raw).hexdigest()!=oid:
            raise Refused('raw object identity differs')
        objects[oid]=(kind,raw)
    def get(oid,kind):
        if oid not in objects or objects[oid][0]!=kind:raise Refused('missing or mistyped graph object')
        return objects[oid][1]
    def tree_entries(oid):
        raw=get(oid,'tree');rows=[]
        while raw:
            head,raw=raw.split(b'\0',1)
            mode,name=head.split(b' ',1)
            link,raw=raw[:32].hex(),raw[32:]
            kind='tree' if mode in (b'40000',b'040000') else 'blob'
            get(link,kind)
            rows.append((mode,kind,link,name))
        return rows
    for oid,(kind,raw) in objects.items():
        if kind=='commit':
            for line in raw.split(b'\n\n',1)[0].splitlines():
                if line.startswith(b'tree '):get(line[5:].decode(),'tree')
                if line.startswith(b'parent '):get(line[7:].decode(),'commit')
        elif kind=='tree':tree_entries(oid)
        elif kind!='blob':raise Refused('unsupported graph object')
    def commit_tree(oid):
        lines=[line[5:].decode() for line in get(oid,'commit').split(b'\n\n',1)[0].splitlines() if line.startswith(b'tree ')]
        if len(lines)!=1:raise Refused('ambiguous tree')
        return lines[0]
    def listing(oid):return b''.join(mode+b' '+kind.encode()+b' '+link.encode()+b'\t'+name+b'\0' for mode,kind,link,name in tree_entries(oid))
    ta,tb=[commit_tree(pins[name]) for name in ('resolver_a','resolver_b')]
    addition=[link for mode,kind,link,name in tree_entries(tb) if name==b'stage2-synthetic-revision']
    if len(addition)!=1:raise Refused('revision missing/ambiguous')
    verify_delta(listing(ta),listing(tb),get(addition[0],'blob'))
    drivers=[oid for mode,kind,oid,name in tree_entries(commit_tree(pins['program'])) if name==b'proof-driver']
    if len(drivers)!=1 or hashlib.sha256(get(drivers[0],'blob')).hexdigest()!=driver:raise Refused('program driver differs')
    # Inspect the complete actual raw returned tree, never a projected file map.
    for row in checked:
        if commit_tree(row['commit'])!=trace['records'][command['step']]['output_hash']:
            raise Refused('full recorded output tree differs')
    return dict(observations=checked,object_count=len(objects),resolver_a_tree=ta,resolver_b_tree=tb,
        qualification='retained actual resolver controls; offline integrity only, NOT stage2 PASS')


def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--batch',type=Path,required=True)
    for name in ('source','driver','bridge','image','endpoint','resolver'):ap.add_argument('--expected-'+name,required=True)
    a=ap.parse_args()
    result=verify(a.batch,**{name:getattr(a,'expected_'+name) for name in ('source','driver','bridge','image','endpoint','resolver')})
    (a.batch/'independent-verification.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result,indent=2))
if __name__=='__main__':main()
