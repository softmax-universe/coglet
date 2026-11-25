"""M-C case contracts and journal-only replay, not runtime qualification."""
import asyncio
import copy
import json
from types import SimpleNamespace
import unittest
from jsonschema import Draft202012Validator
from referencing import Registry, Resource

from build_mc_contracts import outputs
from heard_archive import HEARD_PATH, read_state, replay_heard
from reference_cddl import Schema, ValidationError
import test_abi
from test_abi import check_trace, digest, record_hash
from test_common import SPEC, encode, decode
from test_hub_relay import RelayOracle
from test_surface_case_timeline import Writer, harness


def translate(deadline, source_now, target_now):
    """Recorded simultaneous epoch samples; contract arithmetic, not clock sync."""
    if deadline <= source_now:
        raise ValueError('expired')
    result = target_now + (deadline-source_now)
    if result > 2**64-1:
        raise ValueError('overflow')
    return result


class DispatchContract:
    def __init__(self):
        self.retained = {}

    def map_once(self, slot, intent, source_now, target_now):
        key = (slot, intent['coglet'], intent['id'])
        raw = encode(intent)
        if key in self.retained:
            previous, envelope = self.retained[key]
            if previous != raw:
                raise ValueError('conflict')
            return envelope
        if source_now is None or target_now is None:
            raise ValueError('no mapping')
        action = copy.deepcopy(intent)
        action['deadline'] = translate(action['deadline'], source_now, target_now)
        envelope = encode(action)
        self.retained[key] = (raw, envelope)
        return envelope


class MCContracts(unittest.IsolatedAsyncioTestCase):
    @classmethod
    def setUpClass(cls):
        test_abi.AbiFixtures.setUpClass()
        cls.validators = test_abi.AbiFixtures()
        cls.vectors = json.loads((SPEC/'fixtures/abi/heard-archive.fixture').read_text())

    def validate_wire(self, family, rule, frame):
        schema = json.loads((SPEC/f'messages/{family}.schema.json').read_text())
        common = json.loads((SPEC/'messages/common.schema.json').read_text())
        registry = Registry().with_resources((s['$id'], Resource.from_contents(s)) for s in (schema,common))
        Draft202012Validator({'$ref':schema['$id']+'#/$defs/'+rule},registry=registry).validate(frame)
        cddl=(SPEC/f'messages/{family}.cddl').read_text()+'\n'+(SPEC/'messages/common.cddl').read_text()
        Schema('selected = '+rule+'\n'+cddl).validate_cbor(encode(frame))

    def test_generated_mc_contracts_current(self):
        for path, value in outputs().items():
            self.assertEqual(json.loads((SPEC.parent/path).read_text()), value, path)

    async def frames_through_harness(self, case, frames):
        for corrupt in (False, True):
            values = copy.deepcopy(frames)
            if corrupt:
                # Every case must fail for a wrong result, not just consume bytes.
                last = values[-1]
                if 'valid' in last: last['valid'] = not last['valid']
                elif 'deadline' in last: last['deadline'] += 1
                else: last['result']['next_offset'] += 1
            peer = SimpleNamespace(impl={'transport':'stdio'},writer=Writer(),reader=asyncio.StreamReader())
            for value in values:
                raw = encode(value)
                peer.reader.feed_data(len(raw).to_bytes(4,'big')+raw)
            peer.reader.feed_eof()
            if corrupt:
                with self.assertRaises(AssertionError): await harness.timeline(peer, copy.deepcopy(case))
            else:
                await harness.timeline(peer, copy.deepcopy(case))
                self.assertEqual(await peer.reader.read(),b'')

    async def test_relay_retry_a_after_b_original_success_no_redelivery(self):
        case=json.loads((SPEC.parent/'conformance/hub/event-retry-a-after-b.json').read_text())
        oracle=RelayOracle()
        requests=[s['send'] for s in case['steps'] if 'send' in s]
        frames=[oracle.admit(r) for r in requests]
        self.assertEqual([r['result']['next_offset'] for r in frames],[1,2,1])
        self.assertEqual(oracle.forwarded,requests[0]['params']['events']+requests[1]['params']['events'])
        self.assertEqual(oracle.positions[requests[0]['target']],2)
        bad=copy.deepcopy(requests[2]);bad['params']['events'][0]['payload']={'changed':True}
        self.assertEqual(oracle.admit(bad)['error']['code'],'conflict')
        self.assertEqual(len(oracle.forwarded),2)
        await self.frames_through_harness(case,frames)

    async def test_deadline_domains_abi_surface_and_hub_actions(self):
        vectors=json.loads((SPEC/'fixtures/abi/clock-domains.fixture').read_text())
        for v in vectors:
            with self.subTest(name=v['name']):
                intent=dict(id='4'*32,coglet='coglet:'+'1'*64,capability='speech.stop',
                            args={'stream':'5'*32},deadline=v['deadline'],correlation_id='6'*32)
                original=encode(intent)
                contract=DispatchContract()
                if v['origin']=='hub.actions':
                    hub_wire=contract.map_once('hub.actions',intent,v['source_now'],v['runtime_now'])
                    runtime_intent=decode(hub_wire)
                    request=dict(method='hub.actions',target=intent['coglet'],correlation_id='7'*32,
                                 timeout_ms=2000,params=dict(action=runtime_intent),
                                 context=dict(actor='hub:'+'3'*64))
                    self.validate_wire('hub','hub-actions-request',request)
                    self.assertEqual(runtime_intent['deadline'],v['runtime_deadline'])
                    # A new sender sample must not remap the same hub ID.
                    self.assertEqual(contract.map_once('hub.actions',intent,1,99999999),hub_wire)
                else:
                    runtime_intent=copy.deepcopy(intent)
                mapped=contract.map_once('surface',runtime_intent,v['runtime_now'],v['surface_now'])
                self.assertEqual(decode(mapped)['deadline'],v['surface_deadline'])
                self.assertEqual(encode(intent),original)
                self.assertEqual(runtime_intent['deadline'],v['runtime_deadline'])
                # Even expired/new/missing samples cannot mutate retained bytes.
                self.assertEqual(contract.map_once('surface',runtime_intent,None,None),mapped)
                self.assertEqual(contract.map_once('surface',runtime_intent,2**63,0),mapped)
                changed=copy.deepcopy(runtime_intent);changed['deadline']+=1
                with self.assertRaisesRegex(ValueError,'conflict'):
                    contract.map_once('surface',changed,1000,50000)
                surface_frame=dict(type='act',body=dict(grant_epoch=7,action=decode(mapped)))
                # Exact real surface wire rule, not a generic profile map.
                self.validate_wire('surface','surface-message',surface_frame)
                case=dict(name=v['name'],steps=[dict(send=dict(operation='contract.deadline',vector=v)),
                     dict(match={'deadline':v['surface_deadline']},within_ms=1000)])
                await self.frames_through_harness(case,[dict(deadline=decode(mapped)['deadline'])])
        for deadline,now,target,error in [(0,0,9000,'expired'),(1000,1000,9000,'expired'),
                                         (1500,1000,2**64-1,'overflow')]:
            with self.assertRaisesRegex(ValueError,error): translate(deadline,now,target)
        with self.assertRaisesRegex(ValueError,'no mapping'):
            DispatchContract().map_once('surface',intent,None,None)

    async def test_heard_rehashed_traces_and_harness(self):
        cases=json.loads((SPEC/'fixtures/abi/heard-archive-timeline.fixture').read_text())
        schema=Schema('selected = trace\n'+self.validators.cddl)
        for v,case in zip(self.vectors,cases,strict=True):
            with self.subTest(name=v['name']):
                trace=v['value']
                self.validators.validator('trace').validate(trace)
                schema.validate_cbor(encode(trace))
                self.assertEqual(digest(trace['initial_files']),trace['initial_hash'])
                for rec in trace['records']+([trace['trailing_boundary']] if trace['trailing_boundary'] else []):
                    self.assertEqual(record_hash(rec),rec['record_hash'])
                    self.assertEqual(digest(rec['output_files']),rec['output_hash'])
                if v['valid']:
                    check_trace(trace)
                else:
                    with self.assertRaisesRegex(ValueError,v['error']): check_trace(trace)
                await self.frames_through_harness(case,[dict(valid=v['valid'],dispatched=False,engine_invocations=0)])
                # A weakened implementation that dispatches or runs an engine is rejected.
                for field in ('dispatched','engine_invocations'):
                    peer=SimpleNamespace(impl={'transport':'stdio'},writer=Writer(),reader=asyncio.StreamReader())
                    response=dict(valid=v['valid'],dispatched=False,engine_invocations=0)
                    response[field]=True if field=='dispatched' else 1
                    raw=encode(response);peer.reader.feed_data(len(raw).to_bytes(4,'big')+raw);peer.reader.feed_eof()
                    with self.assertRaises(AssertionError): await harness.timeline(peer,copy.deepcopy(case))

    def test_heard_export_replay_journal_alone(self):
        trace=next(v['value'] for v in self.vectors if v['name']=='heard-export-replay-journal-alone')
        # Export crosses a deterministic byte boundary. Fresh replay has no live
        # state or oracle and recovers intermediate duck and final stopped state.
        archive=encode(trace)
        history,states=replay_heard(decode(archive))
        self.assertEqual([i['text'] for i in history],['Hello world','Hello world','',''])
        self.assertEqual([i['played_ms'] for i in history],[400,300,0,300])
        self.assertEqual(states[1]['items'][1]['heard'],'Hello ')
        self.assertEqual(states[2]['items'][1]['heard'],'Hello world')
        self.assertEqual(trace['records'],[])
        self.assertNotIn('/data/inbox.jsonl',trace['trailing_boundary']['output_files'])
        before_step=next(v['value'] for v in self.vectors if v['name']=='heard-before-reasoning-step')
        history,_=replay_heard(decode(encode(before_step)))
        self.assertEqual(history[1]['text'],'Hello ')
        continuation=next(v['value'] for v in self.vectors if v['name']=='heard-continuation-initial-state')
        self.assertEqual(replay_heard(decode(encode(continuation)))[0],replay_heard(decode(archive))[0])
        capped=next(v['value'] for v in self.vectors if v['name']=='heard-utf8-prefix-cap')
        text=replay_heard(decode(encode(capped)))[0][0]['text']
        self.assertEqual(len(text.encode()),16382)
        self.assertEqual(text,'é'*8191)
        # Unknown full proposal/alignment is not guessed from time ratios.
        self.assertEqual(history[3]['text'],'')

    def test_heard_decoded_state_both_schemas_and_closed_roots(self):
        valid=next(v['value'] for v in self.vectors if v['valid'])
        state=decode(bytes.fromhex(valid['trailing_boundary']['boundary'][0]['after']))
        schema=Schema('selected = heard-state\n'+self.validators.cddl)
        self.validators.validator('heard-state').validate(state)
        schema.validate_cbor(encode(state))
        self.assertFalse(self.validators.validator('trace').is_valid(state))
        for target in ('state','item','span'):
            bad=copy.deepcopy(state)
            obj=bad if target=='state' else bad['items'][0] if target=='item' else bad['items'][0]['spans'][0]
            obj['unknown']=True
            self.assertFalse(self.validators.validator('heard-state').is_valid(bad))
            with self.assertRaises(ValidationError): schema.validate_cbor(encode(bad))
        raw=encode(state)
        # Definite/canonical CBOR only, and initial snapshots are validated too.
        with self.assertRaises(ValueError): read_state((b'\xbf'+raw[1:]+b'\xff').hex())
        trace=copy.deepcopy(valid)
        trace['initial_files'][HEARD_PATH]=encode({'items':[],'injected':True}).hex()
        trace['initial_hash']=digest(trace['initial_files'])
        with self.assertRaisesRegex(ValueError,'heard state: shape'): check_trace(trace)


if __name__=='__main__': unittest.main()
