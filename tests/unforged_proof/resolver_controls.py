"""Strict synthetic retained-client resolver controls; NOT stage2 qualification."""
import json
import re
from hosted_receipts import Refused, validate_invocation

METADATA='stage2-synthetic-revision'

def verify_delta(old,new,metadata):
    def entries(raw):
        result={}
        for row in raw.split(b'\0'):
            if not row:continue
            try:
                descriptor,name=row.split(b'\t',1)
                mode,kind,oid=descriptor.split(b' ')
            except ValueError as e:raise Refused('invalid resolver tree') from e
            if name in result or not re.fullmatch(b'[0-9a-f]{64}',oid):raise Refused('ambiguous resolver tree')
            result[name]=(mode,kind,oid)
        return result
    a,b=entries(old),entries(new)
    key=METADATA.encode()
    if key in a or set(b)!=set(a)|{key} or any(b[k]!=v for k,v in a.items()):
        raise Refused('resolver delta must preserve all original entries')
    if b[key][:2]!=(b'100644',b'blob') or re.fullmatch(b'coglet-stage2-resolver-control-v1\n[0-9a-f]{64}\n',metadata) is None:
        raise Refused('only regular synthetic revision metadata allowed')

def verify_sequence(raw,*,program,inputs,resolver_a,resolver_b,endpoint,image,tree):
    def unique(pairs):
        result={}
        for key,value in pairs:
            if key in result:raise Refused('duplicate observation field')
            result[key]=value
        return result
    try:
        rows=[json.loads(line,object_pairs_hook=unique) for line in raw.splitlines() if line.strip()]
        if len(rows)!=6 or any(not isinstance(r,dict) for r in rows):raise Refused('six ordered observations required')
        if rows[0].get('same_client') is not True or rows[0].get('endpoint_identity')!=endpoint:
            raise Refused('retained authenticated client mismatch')
        if resolver_a==resolver_b:raise Refused('distinct resolver identities required')
        expected=[('a-cold',resolver_a,False),('a-warm',resolver_a,True),('b-cold',resolver_b,False),('b-warm',resolver_b,True),('a-return',resolver_a,True)]
        checked=[]
        for row,(label,resolver,hit) in zip(rows[1:],expected,strict=True):
            if row['label']!=label or row['resolver']!=resolver or row['memo_hit'] is not hit or row['tree']!=tree:
                raise Refused('ordered resolver/cache/oracle mismatch')
            receipt=validate_invocation(raw_commit=row['raw_commit'].encode(),raw_receipt=row['raw_receipt'],commit=row['commit'],output_tree=tree,program_commit=program,input_commit=inputs,resolver=resolver,image=image,memo_hit=hit,fresh=not hit)
            checked.append(dict(label=label,resolver=resolver,commit=row['commit'],tree=tree,receipt=receipt))
        return checked
    except (KeyError,TypeError,ValueError,AttributeError,RecursionError) as e:
        if isinstance(e,Refused):raise
        raise Refused('malformed resolver control transcript') from e
