"""Rebuild DB fixtures/independent JSON schemas; preserve frozen ABI definitions.

CDDL primitive records are hand-authored; JSON primitive records are defined here
independently. Only the unchanged ABI base record fields are mechanically copied.
"""
import argparse
from copy import deepcopy
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'conformance'))
from wire import encode

COMMON = 'https://coglet.dev/spec/messages/common.schema.json#/$defs/'
ABI = 'https://coglet.dev/spec/messages/abi.schema.json#/$defs/'

def ref(name):
    return {'$ref': '#/$defs/' + name}

def common(name):
    return {'$ref': COMMON + name}

def obj(fields, optional=()):
    return dict(type='object', properties=fields, required=[k for k in fields if k not in optional], additionalProperties=False)

def arr(items, maximum, minimum=0):
    return dict(type='array', items=items, minItems=minimum, maxItems=maximum)

def union(*types):
    return {'anyOf': list(types)}

def const(value):
    return {'const': value}

def integer(lo, hi):
    return dict(type='integer', minimum=lo, maximum=hi)

def text(lo, hi=None):
    value = dict(type='string', minLength=lo)
    if hi is not None:
        value['maxLength'] = hi
    return value

U64 = common('uint64')

def schemas():
    abi = json.loads((ROOT / 'spec/messages/abi.schema.json').read_text())
    # Qualify unchanged ABI-local refs when moving definitions into this family.
    def imported(value):
        if isinstance(value, dict):
            return {k: ABI + v[8:] if k == '$ref' and v.startswith('#/$defs/') else imported(v) for k, v in value.items()}
        if isinstance(value, list):
            return [imported(x) for x in value]
        return value
    defs = {}
    defs['db-cell'] = union({'type': 'null'}, integer(-(2**63), 2**64-1), text(0))
    defs['db-cursor'] = text(1)
    defs['db-statement'] = obj(dict(operation=const('statement'), statement=dict(type='string', pattern='^[a-z][a-z0-9_]{0,63}(?![\\s\\S])'), mode={'enum':['read','write']}, params=arr(ref('db-cell'),32), max_rows=integer(1,64), max_bytes=integer(256,65536), cursor=ref('db-cursor')), ('cursor',))
    defs['db-enqueue'] = obj(dict(operation=const('enqueue'), intent_id=common('action-id'), idempotency_key=common('correlation-id'), kind=common('capability-name'), payload=common('profile-map'), correlation_id=common('correlation-id')))
    defs['db-request'] = union(ref('db-statement'),ref('db-enqueue'))
    defs['db-page'] = obj(dict(status=const('page'), columns=arr(text(1),16), rows=arr(arr(ref('db-cell'),16),64), affected=U64, revision=U64, next_cursor=union(ref('db-cursor'),{'type':'null'})))
    defs['db-enqueued'] = obj(dict(status=const('enqueued'),intent_id=common('action-id'),duplicate={'type':'boolean'}))
    defs['db-failed'] = obj(dict(status=const('failed'),error=common('error')))
    defs['db-reply'] = union(*[ref(x) for x in ['db-page','db-enqueued','db-failed']])
    base = dict(intent_id=common('action-id'), idempotency_key=common('correlation-id'), correlation_id=common('correlation-id'))
    defs['db-outcome'] = union(obj(dict(base, status=const('succeeded'), result=common('profile-map'))),obj(dict(base,status={'enum':['failed','cancelled']},error=common('error'))))
    defs['db-host-call'] = union(obj(dict(op=const('db'),argument=ref('db-request'),result=const(0),response=ref('db-reply'))),obj(dict(op=const('db'),argument=ref('db-request'),result=integer(1,4))),obj(dict(op=const('db'),argument_hex={'$ref':ABI+'hex'},result=const(1))))
    defs['db-config'] = obj(dict(schema_min=U64,schema_max=U64,catalog_hash={'$ref':ABI+'hash'}))
    for new, old in [('db-manifest','manifest'),('db-step-record','step-record'),('db-trace','trace'),('db-boundary-record','boundary-record')]:
        value = imported(deepcopy(abi['$defs'][old]))
        branches = value.get('oneOf', value.get('anyOf',[value]))
        for branch in branches:
            props = branch['properties']
            props['version'] = const('0.2')
            extra = {}
            if old == 'manifest':
                extra['database'] = ref('db-config')
            elif old == 'step-record':
                props['calls']['items'] = union({'$ref':ABI+'call'},ref('db-host-call'))
                extra = dict(db_before=U64,db_after=U64,db_schema_version=U64,db_catalog_hash={'$ref':ABI+'hash'})
            elif old == 'trace':
                props['records']['items'] = ref('db-step-record')
                props['trailing_boundary'] = union(ref('db-boundary-record'), {'type':'null'})
                extra = dict(initial_db_revision=U64,final_db_revision=U64)
            props.update(extra)
            branch['required'].extend(extra)
        defs[new]=value
    roots=['db-request','db-reply','db-outcome','db-host-call','db-manifest','db-step-record','db-trace','db-boundary-record']
    defs['database'] = union(*map(ref,roots))
    return dict({'$schema':abi['$schema'],'$id':'https://coglet.dev/spec/messages/database.schema.json','$ref':'#/$defs/database','$defs':defs})


def extended_cddl():
    source = (ROOT/'spec/messages/abi.cddl').read_text().splitlines()
    result=[]
    for new,old in [('db-manifest','manifest'),('db-step-record','step-record'),('db-trace','trace'),('db-boundary-record','boundary-record')]:
        line=next(x for x in source if x.startswith(old+' = '))
        line=new+line[len(old):]
        line=line.replace('version: "0.1"','version: "0.2"').replace('"version": "0.1"','"version": "0.2"')
        if old=='manifest':
            line=line[:-1]+', database: db-config}'
        elif old=='step-record':
            line=line.replace('calls: [* call]', 'calls: [* (call / db-host-call)]')
            line=line.replace('record_hash: hash,','record_hash: hash, db_before: uint64, db_after: uint64, db_schema_version: uint64, db_catalog_hash: hash,')
        elif old=='trace':
            line=line.replace('records": [* step-record]', 'records": [* db-step-record]')
            line=line.replace('trailing_boundary": boundary-record','trailing_boundary": db-boundary-record')
            line=line[:-1]+', "initial_db_revision": uint64, "final_db_revision": uint64}'
        result.append(line)
    path=ROOT/'spec/messages/database.cddl'
    prefix=path.read_text().split('; BEGIN generated ABI extension shapes')[0]
    return prefix+'; BEGIN generated ABI extension shapes\n'+'\n'.join(result)+'\n'


QUERY=dict(operation='statement',statement='objects_page',mode='read',params=['goal'],max_rows=2,max_bytes=1024)
PAGE=dict(status='page',columns=['id','type'],rows=[['a','goal'],['b','goal']],affected=0,revision=7,next_cursor='opaque-owner-revision7-key-b')
ENQUEUE=dict(operation='enqueue',intent_id='1'*32,idempotency_key='2'*32,kind='worker.dispatch',payload={'task':'a'},correlation_id='3'*32)
ENQUEUED=dict(status='enqueued',intent_id='1'*32,duplicate=False)
ERROR=dict(status='failed',error=dict(code='conflict',message='Unit aborted',retryable=False))


def fixtures():
    out=[]
    def add(name,rule,value,valid=True):
        item=dict(name=name,type=rule,valid=valid,value=value)
        if valid: item['cbor_hex']=encode(value).hex()
        out.append(item)
    for name,rule,value in [('bounded-result','db-page',PAGE),('paging-cursor','db-statement',dict(QUERY,cursor=PAGE['next_cursor'])),('query','db-statement',QUERY),('enqueue','db-enqueue',ENQUEUE),('enqueue-first','db-enqueued',ENQUEUED),('duplicate-idempotency-key','db-enqueued',dict(ENQUEUED,duplicate=True)),('transaction-abort','db-failed',ERROR),('recorded-query','db-host-call',dict(op='db',argument=QUERY,result=0,response=PAGE)),('invalid-argument-record','db-host-call',dict(op='db',argument_hex='ff',result=1)),('outcome','db-outcome',dict(intent_id='1'*32,idempotency_key='2'*32,correlation_id='3'*32,status='succeeded',result={}))]:
        add(name,rule,deepcopy(value))
    negatives=[('unbounded-query','db-statement',dict(QUERY,max_rows=65)),('raw-sql','db-statement',dict(QUERY,sql='SELECT 1')),('foreign-owner','db-statement',dict(QUERY,owner='other')),('empty-cursor','db-statement',dict(QUERY,cursor='')),('long-cursor','db-statement',dict(QUERY,cursor='x'*513)),('too-many-rows','db-page',dict(PAGE,rows=[['a','goal']]*65)),('floating-cell','db-page',dict(PAGE,rows=[[1.5,'goal']])),('oversize-cell','db-page',dict(PAGE,rows=[['a'*4097,'goal']])),('unknown-reply-key','db-page',dict(PAGE,extra=True)),('missing-result','db-host-call',dict(op='db',argument=QUERY,response=PAGE)),('response-on-rejection','db-host-call',dict(op='db',argument=QUERY,result=2,response=PAGE)),('bad-intent-key','db-enqueue',dict(ENQUEUE,idempotency_key='bad'))]
    for name,rule,value in negatives:
        add(name,rule,value,False)
        if name in ('long-cursor','oversize-cell'):
            out[-1]['semantic'] = True
    # Every record family has a golden, using existing ABI test records verbatim
    # except the versioned extension fields. This is schema data, not runtime proof.
    base=json.loads((ROOT/'spec/fixtures/abi/cases.json').read_text())
    for old,new in [('manifest','db-manifest'),('step-record','db-step-record'),('trace','db-trace')]:
        candidate=next((deepcopy(c['value']) for c in base if c.get('type')==old and c.get('valid')),None)
        if candidate is None: continue
        candidate['version']='0.2'
        if old=='manifest': candidate['database']=dict(schema_min=1,schema_max=2,catalog_hash='a'*64)
        elif old=='step-record':
            candidate.update(db_before=7,db_after=7,db_schema_version=1,db_catalog_hash='a'*64)
            candidate['calls'].append(dict(op='db',argument=QUERY,result=0,response=PAGE))
        else:
            candidate.update(initial_db_revision=7,final_db_revision=7)
            candidate['records']=[]
        add(new,new,candidate)
    return out


def timelines():
    # The adapter drives its real engine seam. Requests do NOT contain expected
    # results; the harness assertions are an independent contract oracle.
    cases=[]
    for name,script,expected in [
        ('bounded-page',[QUERY],dict(replies=[PAGE],status='committed',db_before=7,db_after=7,dispatch=[])),
        ('paging-cursor',[dict(QUERY,cursor=PAGE['next_cursor'])],dict(replies=[dict(PAGE,rows=[['c','goal']],next_cursor=None)],status='committed',db_before=7,db_after=7,dispatch=[])),
        ('transaction-abort',[ENQUEUE,dict(QUERY,statement='force_conflict')],dict(replies=[ENQUEUED,ERROR],status='aborted',db_before=7,db_after=7,dispatch=[])),
        ('duplicate-key',[ENQUEUE,dict(ENQUEUE,intent_id='4'*32)],dict(replies=[ENQUEUED,dict(ENQUEUED,duplicate=True)],status='committed',db_before=7,db_after=8,dispatch=['1'*32])),
        ('duplicate-key-id-collision',[ENQUEUE,dict(ENQUEUE,intent_id='4'*32,idempotency_key='5'*32),dict(ENQUEUE,intent_id='4'*32)],dict(replies=[ENQUEUED,dict(ENQUEUED,intent_id='4'*32),dict(ENQUEUED,duplicate=True)],status='committed',db_before=7,db_after=8,dispatch=['1'*32,'4'*32])),
        ('conflicting-key',[ENQUEUE,dict(ENQUEUE,payload={'task':'different'})],dict(replies=[ENQUEUED,ERROR],status='aborted',db_before=7,db_after=7,dispatch=[])),
    ]:
        request=dict(operation='db.step',fixture='private-goals-v1',grants=['db.read','db.write','worker.dispatch'],calls=script,termination='committed')
        cases.append(dict(name='db-'+name,peer_role='guest',steps=[{'send':request},{'expect':expected,'within_ms':500}]))
    return cases


def outputs():
    return {'spec/messages/database.schema.json':json.dumps(schemas(),indent=2)+'\n','spec/messages/database.cddl':extended_cddl(),'spec/fixtures/database/cases.json':json.dumps(fixtures(),indent=2)+'\n','conformance/database/cases.json':json.dumps(timelines(),indent=2)+'\n'}


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--check',action='store_true')
    args=parser.parse_args()
    for rel,content in outputs().items():
        path=ROOT/rel
        if args.check:
            if not path.exists() or path.read_text()!=content: raise SystemExit('stale: '+rel)
        else:
            path.parent.mkdir(parents=True,exist_ok=True)
            path.write_text(content)
    print('database: schemas, fixtures and timelines current')

if __name__=='__main__': main()
