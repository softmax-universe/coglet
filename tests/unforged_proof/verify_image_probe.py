#!/usr/bin/env python3
"""Independently reconstruct retained image-control raw graphs. No remote calls.

Requires independent execution pins, not self-certification from command.json.
Synthetic bounded replay only; not strict/private/universal effects proof.
"""
import argparse
import hashlib
import json
from pathlib import Path
from image_controls import verify_sequence,Refused
from wire_trees import object_id,verify_output
from prefix_inputs import step_payload
from standard_step import snapshot_tree,path_parts

def require(value,why):
    if not value:raise Refused(why)

def graph(rows):
    objects={}
    for r in rows:
        if r.get('phase')!='request-object':continue
        require(set(r)=={'phase','oid','kind','hex'} and r['kind'] in ('tree','blob'),'request object envelope')
        raw=bytes.fromhex(r['hex']);kind=r['kind']
        require(object_id(kind,raw)==r['oid'],'request raw hash')
        require(r['oid'] not in objects,'duplicate request graph object')
        objects[r['oid']]=(kind,raw)
    def entries(oid):
        require(oid in objects and objects[oid][0]=='tree','request tree missing')
        raw=objects[oid][1];pos=0;out={};keys=[]
        while pos<len(raw):
            stop=raw.index(b'\0',pos);mode,name=raw[pos:stop].split(b' ',1);name=name.decode()
            require(len(path_parts(name))==1 and name not in out and mode in (b'40000',b'100644',b'100755'),'request path/mode')
            child=raw[stop+1:stop+21].hex();require(len(child)==40,'request child width')
            out[name]=(mode.decode(),child);keys.append((name+('/' if mode==b'40000' else '')).encode());pos=stop+21
        require(keys==sorted(keys),'request tree order')
        return out
    def blob(oid):
        require(oid in objects and objects[oid][0]=='blob','request blob missing');return objects[oid][1]
    def tree256(oid):
        return object_id('tree',b''.join(mode.encode()+b' '+name.encode()+b'\0'+bytes.fromhex(tree256(child) if mode=='40000' else object_id('blob',blob(child),'sha256')) for name,(mode,child) in entries(oid).items()),'sha256')
    def leaves(oid,prefix=''):
        out={}
        for name,(mode,child) in entries(oid).items():
            path=prefix+name
            if mode=='40000':out.update(leaves(child,path+'/'))
            else:out[path]=(mode,blob(child))
        return out
    def closure(oid,depth=0):
        require(depth<=32 and oid in objects,'request graph incomplete/deep')
        result={oid}
        if objects[oid][0]=='tree':
            for mode,child in entries(oid).values():result |= closure(child,depth+1)
        return result
    return objects,entries,blob,tree256,leaves,closure

def verify(batch,*,source,upstream,driver,bridge,endpoint,image_a,image_b):
    info=json.loads((batch/'command.json').read_bytes())
    for key,value in dict(source=source,upstream=upstream,driver=driver,bridge=bridge,endpoint=endpoint,image_a=image_a,image_b=image_b).items():require(info[key]==value,'execution pin '+key)
    trace=json.loads((batch/'trace.json').read_bytes());other=json.loads((batch/'native-rerecorded.json').read_bytes())
    require([r['output_hash'] for r in trace['records']]==[r['output_hash'] for r in other['records']],'native verification trees')
    payload=step_payload(trace,info['step']);require(info['tree']==trace['records'][info['step']]['output_hash'],'oracle output tree')
    commit=(batch/'resolver-commit.raw').read_bytes()
    require(object_id('commit',commit,'sha256')==info['resolver'],'resolver commit hash')
    require(commit.splitlines()[0]==b'tree '+info['resolver_tree'].encode(),'resolver commit tree')
    # No assert: every check remains enabled under Python -O.
    rows=[json.loads(s) for s in (batch/'bridge.jsonl').read_text().splitlines()]
    session=[r for r in rows if r.get('phase')=='image-session']
    require(len(session)==1 and session[0]['endpoint']==endpoint and session[0]['same_client'] is True,'session provenance')
    starts=[i for i,r in enumerate(rows) if r.get('phase')=='image-start'];require(len(starts)==4,'four observations required')
    observations=[];arg_leaves=None;request_oids=[]
    for n,start in enumerate(starts):
        chunk=rows[start:starts[n+1] if n+1<len(starts) else len(rows)]
        obs=[r for r in chunk if r.get('phase')=='image-observation'];require(len(obs)==1,'unique observation');obs=obs[0]
        objects,entries,blob,tree256,leaves,closure=graph(chunk)
        req=obs['request'];top=entries(req)
        require(set(top)=={'base','launcher','resolver','salt','state','worker1'},'exact effective args closure')
        require(top['base'][0]=='100644' and blob(top['base'][1])==('docker://'+obs['image']).encode(),'resolved image request binding')
        require(closure(req)==set(objects),'unreachable extra request object')
        current=leaves(req);current.pop('base')
        if arg_leaves is None:arg_leaves=current
        require(current==arg_leaves,'base-only complete graph invariant')
        effective=b''.join(mode.encode()+b' '+name.encode()+b'\0'+bytes.fromhex(oid) for name,(mode,oid) in top.items() if name!='base')
        require(object_id('tree',effective)==obs['args']==session[0]['args'],'submitted args root')
        bundle_raw=b'40000 args\0'+bytes.fromhex(obs['args'])
        require(object_id('tree',bundle_raw)==obs['bundle']==session[0]['bundle'],'bundle identity')
        resolver=top['resolver'][1]
        require(tree256(resolver)==info['resolver_tree'],'complete resolver tree')
        require(set(leaves(resolver))=={'strategy','drivers/proof'},'closed consumed resolver')
        require(leaves(resolver)['strategy']==('100644',b'driver = proof\ndeterministic = true\n'),'actual selection strategy')
        mode,raw=leaves(resolver)['drivers/proof'];require(mode=='100755' and hashlib.sha256(raw).hexdigest()==driver and raw.startswith(b'\x7fELF'),'consumed executable driver')
        require(top['salt'][0]=='100644' and blob(top['salt'][1])==info['salt'].encode(),'consumed fresh salt')
        state_leaves=leaves(top['state'][1]);require(all(mode=='100644' for mode,raw in state_leaves.values()),'state mode')
        state={p:raw for p,(mode,raw) in state_leaves.items()}
        require(json.loads(state.pop('proof-input.json'))==payload,'genuine full prefix/current input')
        require(snapshot_tree(state)==payload['prefix']['final_hash'],'prior whole state tree')
        http=[r for r in chunk if r.get('phase')=='http'];require(len(http)==3,'exact submit run status graph')
        sub,run,status=http
        require(sub['method']=='POST' and sub['path']=='/submit' and sub['status']==200,'submission request')
        require(json.loads(bytes.fromhex(sub['body_hex']))==dict(base=dict(type='docker',value=obs['image']),bundle=obs['bundle'],salt=info['salt']),'actual submission body')
        require(bytes.fromhex(sub['response_hex']).decode().splitlines()[0]=='tree '+req,'submit response')
        require(run['method']=='GET' and run['path']=='/run?req='+req+('&nocache=1' if n==0 else '') and run['status']==200,'ordinary Bfirst/warm run')
        require(run['headers']==obs['headers'],'actual invocation receipt headers')
        require(bytes.fromhex(run['response_hex']).decode().strip()=='tree '+obs['root'],'run output')
        require(status['method']=='GET' and status['path']=='/status/'+req+'?all=1' and status['status']==200,'authenticated completed status')
        status=json.loads(bytes.fromhex(status['response_hex']))
        sidecars=[r for r in chunk if r.get('phase')=='image-sidecar'];require(len(sidecars)==1,'unique sidecar')
        side=sidecars[0];raw=bytes.fromhex(side['hex'])
        require(object_id('blob',raw)==side['oid'] and status['arg_tree']==req and status['out_trace']==[side['oid']],'status sidecar request binding')
        require(json.loads(raw)==dict(salt=info['salt'],mode='bounded-native-replay'),'sidecar salt/mode')
        tree=verify_output(chunk,obs['root'])
        observations.append({k:obs[k] for k in ['label','request','args','bundle','resolver','driver','salt','image','headers']}|dict(tree=tree))
        request_oids.append(req)
    summary=verify_sequence(observations,image_a=image_a,image_b=image_b,resolver=info['resolver'],driver=driver,salt=info['salt'],tree=info['tree'])
    return dict(summary=summary,observations=observations,unique_requests=len(set(request_oids)),qualification=info['qualification'])

def main():
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument('--batch',type=Path,required=True)
    for name in ['source','upstream','driver','bridge','endpoint','image-a','image-b']:ap.add_argument('--'+name,required=True)
    a=ap.parse_args();print(json.dumps(verify(a.batch,source=a.source,upstream=a.upstream,driver=a.driver,bridge=a.bridge,endpoint=a.endpoint,image_a=a.image_a,image_b=a.image_b),indent=2))
if __name__=='__main__':main()
