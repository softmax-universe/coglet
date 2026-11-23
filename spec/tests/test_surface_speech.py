"""Executable fixture-data checks, not a surface implementation or acoustic proof."""
import copy
import json
from pathlib import Path
import os
import struct
import subprocess
import tempfile
import unittest

import cbor2
from jsonschema import Draft202012Validator
from pycddl import Schema, ValidationError
from referencing import Registry, Resource

from test_common import SPEC, encode, decode
import build_surface_fixtures as build
import build_surface_cases as case_build

ROOT = SPEC.parent
EVENTS = {'speech.start':'speech-start', 'speech.partial':'speech-partial',
          'speech.end':'speech-end', 'speech.position':'speech-position',
          'speech.barge_in':'speech-barge-in', 'speech.tool_call':'tool-call',
          'action.outcome':'action-outcome'}
ACTIONS = {'speech.speak_chunk':'speak-chunk', 'speech.stop':'stop',
           'speech.backchannel':'backchannel', 'speech.session_open':'session-open',
           'speech.session_close':'session-close', 'speech.tool_result':'tool-result',
           'expression.set_expression':'set-expression'}


class ProfileFixtures(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.schemas = {name:json.loads((SPEC/f'messages/{name}.schema.json').read_text())
                       for name in ('common','surface','speech','expression')}
        cls.registry = Registry().with_resources((v['$id'],Resource.from_contents(v)) for v in cls.schemas.values())
        cls.common_cddl = (SPEC/'messages/common.cddl').read_text()
        cls.target = ROOT/'target'
        cls.target.mkdir(exist_ok=True)
        cls.ruby_env=dict(os.environ, BUNDLE_GEMFILE=str(SPEC/'tests/Gemfile'))
        cls.temp = tempfile.TemporaryDirectory(prefix='surface-schemas-', dir=cls.target)
        cls.addClassCleanup(cls.temp.cleanup)
        cls.cddl_paths={}
        for profile in cls.schemas:
            text = '' if profile=='common' else (SPEC/f'messages/{profile}.cddl').read_text()
            path=Path(cls.temp.name)/(profile+'.cddl')
            path.write_text(text+'\n'+cls.common_cddl)
            cls.cddl_paths[profile]=path

    def validate(self, profile, rule, value):
        schema = self.schemas[profile]
        validator = Draft202012Validator({'$ref':schema['$id']+'#/$defs/'+rule}, registry=self.registry)
        js_valid = validator.is_valid(value)
        checked=subprocess.run(['ruby','-rbundler/setup',str(SPEC/'tests/cddl_check.rb'),rule,
                                str(self.cddl_paths[profile])], input=json.dumps(value).encode(),
                               env=self.ruby_env, capture_output=True, timeout=10)
        self.assertIn(checked.returncode,(0,1),checked.stderr.decode())
        cddl_valid=checked.returncode==0
        self.assertEqual(js_valid,cddl_valid,(profile,rule,value,checked.stderr.decode()))
        return js_valid

    def wire(self, msg):
        self.assertTrue(self.validate('surface','surface-message',msg),msg)
        body=msg['body']
        if msg['type']=='event' and body['kind'] in EVENTS:
            profile='common' if body['kind']=='action.outcome' else 'speech'
            self.assertTrue(self.validate(profile,EVENTS[body['kind']],body['payload']))
        if msg['type']=='act' and body['action']['capability'] in ACTIONS:
            cap=body['action']['capability']
            profile='expression' if cap.startswith('expression.') else 'speech'
            self.assertTrue(self.validate(profile,ACTIONS[cap],body['action']['args']))

    def test_dual_schema_goldens(self):
        total=valid=0
        for profile in ('surface','speech','expression'):
            Draft202012Validator.check_schema(self.schemas[profile])
            cases=json.loads((SPEC/f'fixtures/{profile}/cases.json').read_text())
            self.assertEqual(len(cases),len({c['name'] for c in cases}))
            for case in cases:
                with self.subTest(profile=profile,case=case['name']):
                    total+=1
                    self.assertEqual(self.validate(profile,case['type'],case['value']),case['valid'])
                    if case['valid']:
                        valid+=1
                        raw=bytes.fromhex(case['cbor_hex'])
                        self.assertEqual(decode(raw),case['value'])
                        self.assertEqual(encode(case['value']),raw)
        print(f'surface/speech/expression: {total} dual-schema cases; {valid} golden round-trips')

    def test_generated_files_current(self):
        for path,content in dict(build.outputs(), **case_build.outputs()).items():
            self.assertEqual((ROOT/path).read_text(),content,path)

    def test_boundaries_and_unicode(self):
        for n,valid in [(0,False),(1,True),(4096,True),(4097,False)]:
            args=dict(build.values['speech']['speak-chunk'],text='a'*n)
            self.assertEqual(self.validate('speech','speak-chunk',args),valid)
        args=dict(build.values['speech']['speak-chunk'],text='é\n你好')
        self.assertTrue(self.validate('speech','speak-chunk',args))
        for value in [-1,0,1000,1001]:
            args=dict(build.values['expression']['set-expression'],intensity=value)
            self.assertEqual(self.validate('expression','set-expression',args),0<=value<=1000)
        # Shape acceptance does not waive the additional UTF-8 byte budget.
        args=dict(build.values['speech']['speak-chunk'],text='é'*4096)
        self.assertTrue(self.validate('speech','speak-chunk',args))
        self.assertGreater(len(args['text'].encode()),4096)

    def test_transport_vectors(self):
        vectors=json.loads((SPEC/'fixtures/surface/framing.json').read_text())
        for vector in vectors:
            raw=bytes.fromhex(vector['hex'])
            with self.subTest(vector=vector['name']):
                try:
                    if len(raw)<4: raise ValueError('short header')
                    n=struct.unpack('!I',raw[:4])[0]
                    if not 0<n<=1048576 or len(raw)!=n+4: raise ValueError('length')
                    self.wire(decode(raw[4:]))
                    accepted=True
                except (ValueError,cbor2.CBORDecodeError):
                    accepted=False
                self.assertEqual(accepted,vector['valid'])
        # Fragmentation and coalescing must not change the parsed items.
        msg={'type':'hello','body':build.values['surface']['hello']}
        raw=encode(msg); frame=struct.pack('!I',len(raw))+raw
        buffer=b''; items=[]
        for fragment in [frame[:1],frame[1:3],frame[3:8],frame[8:]+frame]:
            buffer+=fragment
            while len(buffer)>=4:
                n=struct.unpack('!I',buffer[:4])[0]
                if len(buffer)<n+4: break
                items.append(decode(buffer[4:n+4])); buffer=buffer[n+4:]
        self.assertEqual(items,[msg,msg]); self.assertEqual(buffer,b'')
        self.assertEqual(decode(raw),msg)  # ABI payload has no socket prefix.

    def test_native_audio_timelines(self):
        timelines=json.loads((SPEC/'fixtures/speech/timelines.json').read_text())
        self.assertEqual({t['name'] for t in timelines},
                         {'barge-in-stop','over-talk-duck','late-partial-and-stale-revision'})
        for timeline in timelines:
            with self.subTest(timeline=timeline['name']):
                utterances={}; closed=set(); emitted=[]; dropped=0; output='idle'; terminal=None
                last_time=-1; seq=0; stream=None
                for step in timeline['steps']:
                    self.assertGreaterEqual(step['at_ms'],last_time); last_time=step['at_ms']
                    kind=step['input']; payload={k:v for k,v in step.items() if k not in ('input','at_ms')}
                    normalized=[]
                    if kind=='start':
                        self.assertNotIn(step['utterance'],utterances)
                        utterances[step['utterance']]=0
                        normalized=[('speech.start',payload)]
                    elif kind in ('partial','end'):
                        uid=step['utterance']; self.assertIn(uid,utterances)
                        if uid in closed or step['revision']<=utterances[uid]:
                            dropped+=1; continue
                        utterances[uid]=step['revision']
                        if kind=='end': closed.add(uid)
                        normalized=[('speech.'+kind,payload)]
                    elif kind in ('play','finish'):
                        stream=step['stream']; output='playing' if kind=='play' else 'finished'
                        normalized=[('speech.position',dict(payload,position_ms=payload.get('position_ms',0),state=output))]
                        if kind=='finish': terminal='succeeded'
                    elif kind=='barge_in':
                        self.assertEqual(step['stream'],stream); self.assertIn(step['utterance'],utterances)
                        output='stopped' if timeline['config']['barge']=='stop' else 'ducked'
                        normalized=[('speech.barge_in',dict(payload,reflex=output)),
                                    ('speech.position',{'stream':stream,'chunk':0,'item':step['item'],'played_ms':step['played_ms'],
                                                        'position_ms':step['position_ms'],'state':output})]
                        if output=='stopped': terminal='cancelled'
                    else: self.fail(kind)
                    for name,body in normalized:
                        self.wire(build.event(name,body,seq,step['at_ms']*1000)); seq+=1
                        emitted.append(name)
                    if kind=='finish' or (kind=='barge_in' and output=='stopped'):
                        outcome={'id':build.cid,'correlation_id':build.cid,'status':terminal}
                        outcome.update({'result':{}} if terminal=='succeeded' else {'error':dict(build.err,code='cancelled')})
                        self.wire(build.event('action.outcome',outcome,seq,step['at_ms']*1000)); seq+=1
                        emitted.append('action.outcome')
                self.assertEqual({'events':emitted,'discarded':dropped,'output':output,'terminal':terminal},timeline['expected'])

    def test_conformance_wire_case_data(self):
        count=0
        for suite in ('surface','speech'):
            for path in sorted((ROOT/f'conformance/{suite}').glob('*.json')):
                case=json.loads(path.read_text())
                self.assertEqual(set(case),{'name','peer_role','fixtures','steps'},path)
                self.assertIn(case['peer_role'],('runtime','surface'))
                self.assertTrue(case['fixtures'])
                self.assertTrue(case['name']); self.assertTrue(case['steps'])
                last_time=0
                for step in case['steps']:
                    self.assertTrue(set(step)<={'send','expect','match','silence_ms','within_ms','at_ms'},path)
                    self.assertEqual(sum(k in step for k in ('send','expect','match','silence_ms')),1)
                    if 'at_ms' in step:
                        self.assertGreaterEqual(step['at_ms'],last_time); last_time=step['at_ms']
                    if 'send' in step: self.wire(step['send'])
                    if 'expect' in step: self.wire(step['expect'])
                    if 'match' in step:
                        self.assertTrue(step['match']); self.assertGreater(step['within_ms'],0)
                count+=1
        self.assertGreaterEqual(count,10)
        print(f'surface/speech: {count} executable harness case records validated (not implementation conformance)')


if __name__=='__main__': unittest.main()
