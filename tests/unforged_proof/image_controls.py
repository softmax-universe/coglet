"""Strict LOCAL verifier for the synthetic existing-image control, no sandbox claim."""
import re
from standard_step import Refused

HEX40=re.compile('[0-9a-f]{40}')
HEX64=re.compile('[0-9a-f]{64}')
IMAGE=re.compile(r'localhost:5000/caos@sha256:[0-9a-f]{64}')

def verify_headers(headers,image,hit):
    if not isinstance(headers,list) or not isinstance(image,str) or IMAGE.fullmatch(image) is None:
        raise Refused('exact image and header pairs required')
    values={}
    for pair in headers:
        if not isinstance(pair,(list,tuple)) or len(pair)!=2 or not all(isinstance(s,str) for s in pair):
            raise Refused('header pair required')
        name,value=pair
        name=name.lower()
        if name in ('x-caos-cache','x-caos-image'):
            if name in values:raise Refused('ambiguous execution receipt header')
            values[name]=value
    if values!={'x-caos-image':image,'x-caos-cache':'hit' if hit else 'miss'}:
        raise Refused('cache execution or resolved image mismatch/missing')
    return 'caos' if hit else 'none'

def verify_sequence(rows,*,image_a,image_b,resolver,driver,salt,tree):
    if (not isinstance(rows,list) or len(rows)!=4 or image_a==image_b
        or any(not isinstance(s,str) or HEX64.fullmatch(s) is None for s in (resolver,driver,salt,tree))):
        raise Refused('exact four-step image sequence and independent pins required')
    for i,(row,label,image,hit) in enumerate(zip(rows,['a-cold','b-first','b-warm','a-return'],
            [image_a,image_b,image_b,image_a],[False,False,True,True])):
        if not isinstance(row,dict) or set(row)!={'label','request','args','bundle','resolver','driver','salt','image','headers','tree'}:
            raise Refused('closed observation required')
        if row['label']!=label or row['image']!=image or (row['resolver'],row['driver'],row['salt'],row['tree'])!=(resolver,driver,salt,tree):
            raise Refused('ordered observation or source/output identity mismatch')
        if any(not isinstance(row[k],str) or HEX40.fullmatch(row[k]) is None for k in ['request','args','bundle']):
            raise Refused('raw request object identity required')
        verify_headers(row['headers'],image,hit)
        if i and any(row[k]!=rows[0][k] for k in ['args','bundle']):
            raise Refused('only base may change')
    if rows[0]['request']!=rows[3]['request'] or rows[1]['request']!=rows[2]['request'] or rows[0]['request']==rows[1]['request']:
        raise Refused('image request alias or wrong return entry')
    return dict(executions=2,remote_hits=2)
