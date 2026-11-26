"""Database extension schema and executable case-contract checks, NOT a DB host.

The scripted reader below is intentionally only a harness oracle. No SQLite,
engine, dispatcher or migration qualification is claimed by these tests.
"""
import asyncio
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest

import cbor2
from jsonschema import Draft202012Validator
from referencing import Registry, Resource
from reference_cddl import Schema, ValidationError
from build_database import outputs, QUERY, PAGE

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT/'conformance'))
from harness import timeline, validate_case
from wire import encode, decode


def byte_bounds(value):
    """Protocol semantic byte caps, kept outside character-based JSON shapes."""
    if isinstance(value, dict):
        for key,item in value.items():
            if key in ('cursor','next_cursor') and isinstance(item,str) and len(item.encode())>512:
                raise ValueError('cursor bytes')
            if key=='columns' and any(len(x.encode())>64 for x in item):
                raise ValueError('column bytes')
            if key in ('rows','params'):
                def cell(v):
                    if isinstance(v,list):
                        for x in v: cell(x)
                    elif isinstance(v,str) and len(v.encode())>4096:
                        raise ValueError('cell bytes')
                cell(item)
            byte_bounds(item)
    elif isinstance(value,list):
        for item in value: byte_bounds(item)


def page_contract(request, reply):
    byte_bounds(request)
    byte_bounds(reply)
    if reply['status']!='page': return
    if len(encode(reply))>request['max_bytes'] or len(reply['rows'])>request['max_rows']:
        raise ValueError('page bound')
    if len(set(reply['columns']))!=len(reply['columns']):
        raise ValueError('duplicate columns')
    if any(len(row)!=len(reply['columns']) for row in reply['rows']):
        raise ValueError('row arity')
    if request['mode']=='write' and (reply['columns'] or reply['rows'] or reply['next_cursor'] is not None):
        raise ValueError('write returned rows')
    if request['mode']=='read' and reply['affected']!=0: raise ValueError('read affected')
    if reply['next_cursor'] is not None and not reply['rows']: raise ValueError('empty nonterminal page')


def step_contract(request, observed):
    """Check declared engine-seam outcomes, not simulate execution or synthesize them."""
    if len(observed['replies'])!=len(request['calls']): raise ValueError('reply count')
    poisoned=any(x['status']=='failed' for x in observed['replies'])
    if 'recovery' in request:
        recovery=observed['recovery']
        marker=request['recovery']['crash']=='after_db_commit'
        prepared=request['recovery']['prepared_record_hash']
        if recovery != dict(marker_present=marker,journal_published=True,guest_return=0,guest_effects_published=marker,prepared_record_hash=prepared):
            raise ValueError('recovery observations')
        if poisoned or request['termination']!='committed': raise ValueError('unsuccessful preparation')
        if observed['status']!=('committed' if marker else 'aborted'): raise ValueError('marker decision')
        if marker:
            if 'failure' in observed: raise ValueError('committed failure')
        elif observed.get('failure')!=dict(kind='db_recovery_abort',prepared_record_hash=prepared):
            raise ValueError('recovery failure binding')
    if poisoned and observed['status']!='aborted': raise ValueError('poison must abort')
    if observed['status']=='aborted':
        if observed['db_after']!=observed['db_before'] or observed['dispatch']:
            raise ValueError('abort leaked effects')
    inserted={}
    for call,reply in zip(request['calls'],observed['replies']):
        if call['operation']=='statement':
            page_contract(call,reply)
        elif reply['status']=='enqueued':
            key=call['idempotency_key']
            if key in inserted:
                first=inserted[key]
                if any(call[k]!=first[k] for k in ('kind','payload','correlation_id')):
                    raise ValueError('conflicting duplicate accepted')
                if not reply['duplicate'] or reply['intent_id']!=first['intent_id']:
                    raise ValueError('duplicate changed identity')
            else:
                if reply['duplicate']: raise ValueError('unseeded duplicate')
                inserted[key]=call
    expected=[call['intent_id'] for call in inserted.values()] if observed['status']=='committed' else []
    if observed['dispatch']!=expected: raise ValueError('dispatch identity/count')
    writes=bool(inserted) or any(c['operation']=='statement' and c['mode']=='write' and r['status']=='page' for c,r in zip(request['calls'],observed['replies']))
    delta=1 if writes and observed['status']=='committed' else 0
    if observed['db_after']!=observed['db_before']+delta: raise ValueError('revision')


def recovery_replay_contract(record, execution):
    """Archive-only case oracle, not guest execution or physical recovery."""
    if record['version']!='1.1' or record['status']!='aborted' or record['failure']['kind']!='db_recovery_abort':
        raise ValueError('not a recovery abort')
    if execution['guest_return']!=0 or execution['calls']!=record['calls']:
        raise ValueError('replay divergence')
    for call in record['calls']:
        if call['op']=='db' and (call['result']!=0 or call['response']['status']=='failed'):
            raise ValueError('poisoned preparation')
    if record['output_files']!=record['input_files'] or record['output_hash']!=record['input_hash'] or record['db_after']!=record['db_before']:
        raise ValueError('abort state')
    if any(record[k] for k in ('actions','timers','llm_requests','logs')):
        raise ValueError('abort effects')


class Sink:
    def __init__(self): self.raw=bytearray()
    def write(self,data): self.raw.extend(data)
    async def drain(self): pass


async def run_scripted(case, frames):
    reader=asyncio.StreamReader()
    for frame in frames:
        raw=encode(frame)
        reader.feed_data(len(raw).to_bytes(4,'big')+raw)
    reader.feed_eof()
    peer=SimpleNamespace(impl={'transport':'stdio'},reader=reader,writer=Sink())
    await timeline(peer,case)
    # The reader must consume all frames, not silently ignore an extra outcome.
    if await reader.read(): raise AssertionError('unconsumed frame')
    return bytes(peer.writer.raw)


class DatabaseContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        paths=[ROOT/'spec/messages'/f'{x}.schema.json' for x in ('common','abi','database')]
        cls.schemas=[json.loads(p.read_text()) for p in paths]
        cls.registry=Registry().with_resources((s['$id'],Resource.from_contents(s)) for s in cls.schemas)
        cls.schema=cls.schemas[-1]
        for schema in cls.schemas: Draft202012Validator.check_schema(schema)
        cls.cddl='\n'.join((ROOT/'spec/messages'/f'{x}.cddl').read_text() for x in ('database','abi','common'))
        cls.cases=json.loads((ROOT/'spec/fixtures/database/cases.json').read_text())
        cls.timelines=json.loads((ROOT/'conformance/database/cases.json').read_text())

    def valid(self,rule,value):
        selected={'$ref': self.schema['$id']+'#/$defs/'+rule}
        js=Draft202012Validator(selected,registry=self.registry).is_valid(value)
        try:
            Schema('entry = '+rule+'\n'+self.cddl).validate_cbor(cbor2.dumps(value))
            cd=True
        except ValidationError:
            cd=False
        return js,cd

    def test_freshness_and_registry(self):
        for rel,content in outputs().items():
            self.assertEqual((ROOT/rel).read_text(),content,rel)
        contracts=json.loads((ROOT/'codegen/contracts.json').read_text())
        self.assertEqual(contracts['database'],'generated')

    def test_dual_schemas_and_goldens(self):
        self.assertEqual(len(self.cases),len({x['name'] for x in self.cases}))
        for case in self.cases:
            with self.subTest(case=case['name']):
                expected=case['valid'] or case.get('semantic',False)
                self.assertEqual(self.valid(case['type'],case['value']),(expected,expected))
                if case['valid']:
                    raw=bytes.fromhex(case['cbor_hex'])
                    self.assertEqual(encode(case['value']),raw)
                    self.assertEqual(decode(raw),case['value'])
                if case.get('semantic'):
                    with self.assertRaises(ValueError): byte_bounds(case['value'])
        print(f'database: {len(self.cases)} dual-schema cases; {sum(x["valid"] for x in self.cases)} golden encodings')

    def test_roots_and_nested_unknown_fields(self):
        for case in self.cases:
            if not case['valid']: continue
            self.assertEqual(self.valid('database',case['value']),(True,True),case['name'])
        for rule,value in [('db-request',dict(QUERY,owner='other')),('db-reply',dict(PAGE,extra=[])),('db-host-call',dict(op='db',argument=dict(QUERY,owner='other'),result=0,response=PAGE))]:
            self.assertEqual(self.valid(rule,value),(False,False))
            self.assertEqual(self.valid('database',value),(False,False))

    def test_utf8_and_encoded_bounds(self):
        # Boundary is bytes, including CBOR overhead, not string characters.
        for text in ['x'*4096,'é'*2048,'🙂'*1024]:
            byte_bounds(dict(rows=[[text]]))
            with self.assertRaises(ValueError): byte_bounds(dict(rows=[[text+'x']]))
        byte_bounds(dict(cursor='é'*256))
        with self.assertRaises(ValueError): byte_bounds(dict(cursor='é'*257))
        control=dict(PAGE,rows=[['a','goal']],next_cursor=None)
        request=dict(QUERY,max_rows=1,max_bytes=256)
        page_contract(request,control)
        for bad in [dict(control,rows=[['a']]),dict(control,columns=['id','id']),dict(control,rows=[],next_cursor='more'),dict(control,rows=[['a','x'*256]])]:
            with self.assertRaises(ValueError): page_contract(request,bad)

    def test_timelines_and_actual_harness_reader(self):
        for case in self.timelines:
            with self.subTest(case=case['name']):
                validate_case(case)
                request=case['steps'][0]['send']
                response=case['steps'][1]['expect']
                step_contract(request,response)
                raw=asyncio.run(run_scripted(case,[response]))
                self.assertEqual(decode(raw[4:]),request)
                for mutation in [dict(response,db_after=999),dict(response,dispatch=['f'*32])]:
                    with self.assertRaises(ValueError): step_contract(request,mutation)
                    with self.assertRaises(AssertionError): asyncio.run(run_scripted(case,[mutation]))
                with self.assertRaises(AssertionError): asyncio.run(run_scripted(case,[]))
                # A weakened expected dispatch count would fail against a compliant peer.
                weak=deepcopy(case)
                weak['steps'][1]['expect']['dispatch']=['f'*32]
                with self.assertRaises(AssertionError): asyncio.run(run_scripted(weak,[response]))

    def test_poison_and_duplicate_negative_controls(self):
        aborted=next(c for c in self.timelines if c['name']=='db-transaction-abort')
        bad=deepcopy(aborted['steps'][1]['expect'])
        bad.update(status='committed',db_after=8,dispatch=['1'*32])
        with self.assertRaises(ValueError): step_contract(aborted['steps'][0]['send'],bad)
        dup=next(c for c in self.timelines if c['name']=='db-duplicate-key')
        bad=deepcopy(dup['steps'][1]['expect'])
        bad['replies'][1]['intent_id']='4'*32
        with self.assertRaises(ValueError): step_contract(dup['steps'][0]['send'],bad)

    def test_recovery_outcome_negative_controls(self):
        for case in self.timelines:
            request=case['steps'][0]['send']
            if 'recovery' not in request: continue
            response=case['steps'][1]['expect']
            mutations=[]
            for key,value in response['recovery'].items():
                bad=deepcopy(response)
                bad['recovery'][key]=not value if isinstance(value,bool) else (1 if key=='guest_return' else 'c'*64)
                mutations.append(bad)
            bad=deepcopy(response)
            bad['status']='aborted' if response['status']=='committed' else 'committed'
            mutations.append(bad)
            bad=deepcopy(response)
            bad['failure']=dict(kind='db_recovery_abort',prepared_record_hash='c'*64)
            mutations.append(bad)
            bad=deepcopy(response)
            bad['replies'][0]=dict(status='failed',error=dict(code='conflict',message='fail',retryable=False))
            mutations.append(bad)
            if 'failure' in response:
                bad=deepcopy(response)
                del bad['failure']
                mutations.append(bad)
            for bad in mutations:
                with self.assertRaises(ValueError): step_contract(request,bad)
                with self.assertRaises(AssertionError): asyncio.run(run_scripted(case,[bad]))

    def test_frozen_failure_remains_closed(self):
        value=dict(kind='db_recovery_abort',prepared_record_hash='b'*64)
        abi=self.schemas[1]
        self.assertFalse(Draft202012Validator({'$ref':abi['$id']+'#/$defs/failure'},registry=self.registry).is_valid(value))
        with self.assertRaises(ValidationError):
            Schema('entry = failure\n'+self.cddl).validate_cbor(cbor2.dumps(value))

    def test_archive_recovery_abort_contract(self):
        record=deepcopy(next(c['value'] for c in self.cases if c['name']=='recovery-abort'))
        execution=dict(guest_return=0,calls=deepcopy(record['calls']))
        recovery_replay_contract(record,execution)
        for mutation in [dict(execution,guest_return=1),dict(execution,calls=[]),dict(execution,calls=execution['calls']*2)]:
            with self.assertRaises(ValueError): recovery_replay_contract(record,mutation)
        for field,value in [('db_after',8),('output_hash','c'*64),('output_files',{'/data/leak':'00'}),('status','committed')]+[(k,[{}]) for k in ('actions','timers','llm_requests','logs')]:
            with self.assertRaises(ValueError): recovery_replay_contract(dict(record,**{field:value}),execution)
        bad=deepcopy(record)
        call=next(c for c in bad['calls'] if c['op']=='db')
        call['response']=dict(status='failed',error=dict(code='conflict',message='fail',retryable=False))
        with self.assertRaises(ValueError): recovery_replay_contract(bad,dict(guest_return=0,calls=bad['calls']))

if __name__=='__main__': unittest.main()
