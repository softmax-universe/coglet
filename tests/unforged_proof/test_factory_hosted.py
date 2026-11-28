"""Service seam unit tests with explicit fake native/transport; NOT hosted proof."""
import copy
from contextlib import closing
import json
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest
from unittest.mock import patch
ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT/'crates/coglet-gate/python'),str(ROOT/'sim'),str(ROOT/'sdk/surface-python')]
from coglet_gate import ArtifactStore, Binding, BuildIssuer, GateService, GateVerifier, Refused, Suite, TrustKey, canonical, digest
from coglet_gate.hosted import step_salt
from coglet_sim.evaluation import NativeTools

class FakeHosted:
    """Trusted transport fake, not remote evidence or a real receipt."""
    identity = {'implementation_sha256':'a'*64,'configuration_sha256':'b'*64,
                'worker_image':'native@sha256:'+'c'*64,'endpoint_identity':'d'*64}
    def evaluate(self,work,request,archive,*,attempt='7'*64):
        return [dict(index=i,salt=step_salt(request,i,attempt),request_oid=hex(i+1)[2:].zfill(40),
                     tree=r['output_hash'],worker_image=self.identity['worker_image'],
                     transcript_sha256='e'*64) for i,r in enumerate(archive['records'])]

class FactoryHostedTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.store = ArtifactStore(self.root/'artifacts')
        self.bk,self.gk = TrustKey('build',b'b'*32),TrustKey('gate',b'g'*32)
        self.wasm = b'unit-fake-wasm'
        self.binding = Binding('coglet:'+'1'*64,'fixture','2'*40,'3'*40,'4'*32,digest(self.wasm))
        self.suite = Suite('fixture','gridbot',42,2,1)
        self.build = BuildIssuer(self.store,self.bk).fixture(self.binding,self.wasm,b'explicit unit fake')
        paths=[]
        for name in ('daemon','proof'):
            p=self.root/name;p.write_bytes(b'not executable native code');p.chmod(0o700);paths.append(p)
        self.hosted=FakeHosted()
        self.service=GateService(self.store,self.bk,self.gk,NativeTools(*paths),hosted=self.hosted)
        self.verifier=GateVerifier(self.root/'challenges.sqlite',self.store,self.bk,self.gk,tools=self.service.hosted_tool_identity)
        self.request=self.verifier.challenge_fixture(self.binding,self.suite,self.build)
        self.archive=dict(coglet=self.binding.coglet,records=[dict(output_hash='5'*64),dict(output_hash='6'*64)])
        self.report=dict(world='gridbot',seed=42,max_steps=2,minimum_actions=1,coglet=self.binding.coglet,
            rewritten_sha256=digest(self.wasm),independent_guest_reexecution=True,passed=False,failure='unit-behavior-reject')
        def native(work,tools,rewritten,**kw):
            Path(kw.pop('archive')).write_bytes(canonical(self.archive))
            return copy.deepcopy(self.report)
        self.native=patch('coglet_sim.evaluation.evaluate',side_effect=native)
        self.native.start();self.addCleanup(self.native.stop)
    def evaluate(self):
        with patch('coglet_gate.secrets.token_hex',return_value='7'*64):
            return self.service.evaluate_hosted(self.root,self.request,self.wasm)
    def no_publication(self):
        for path in self.store.root.iterdir():
            self.assertNotEqual(json.loads(path.read_bytes())['kind'],'evaluation')
        with closing(sqlite3.connect(self.root/'challenges.sqlite')) as db:
            self.assertIsNone(db.execute('SELECT artifact FROM challenges WHERE nonce=?',(self.request['nonce'],)).fetchone()[0])
    def test_actual_service_sequence_preserves_native_behavior_and_one_use(self):
        ref=self.evaluate();value=self.verifier.accept_fixture(self.request,ref)
        self.assertIs(value['report']['passed'],False)
        self.assertEqual(len(value['report']['hosted']['steps']),2)
        self.assertEqual(value['qualification'],'fixture-only')
        with self.assertRaises(Refused):self.verifier.accept_fixture(self.request,ref)
        with self.assertRaises(Refused):self.verifier.verify_production(self.request,ref)
    def test_transport_failure_no_signature_unused_challenge_and_retry(self):
        with patch.object(self.hosted,'evaluate',side_effect=TimeoutError('unit timeout')):
            with self.assertRaises(Refused):self.evaluate()
        self.no_publication()
        self.verifier.accept_fixture(self.request,self.evaluate())
    def test_incomplete_ambiguous_reordered_stale_and_forged_results_refuse(self):
        good=self.hosted.evaluate(self.root,self.request,self.archive)
        cases=[[],good[:1],good+good[:1],list(reversed(good))]
        for key,value in [('tree','f'*64),('salt','f'*64),('worker_image','native@sha256:'+'f'*64),('passed',True),('request_oid','short')]:
            bad=copy.deepcopy(good);bad[0][key]=value;cases.append(bad)
        for bad in cases:
            with self.subTest(bad=bad),patch.object(self.hosted,'evaluate',return_value=bad):
                with self.assertRaises(Refused):self.evaluate()
                self.no_publication()
    def test_changed_transport_identity_refuses_before_callback(self):
        self.hosted.identity={**self.hosted.identity,'configuration_sha256':'f'*64}
        with patch.object(self.hosted,'evaluate',side_effect=AssertionError('called')):
            with self.assertRaises(Refused):self.evaluate()
        self.no_publication()
    def test_changed_nonce_changes_each_consumed_salt_and_copied_result_refuses(self):
        good=self.hosted.evaluate(self.root,self.request,self.archive)
        other=self.verifier.challenge_fixture(self.binding,self.suite,self.build)
        self.assertNotEqual(step_salt(other,0,'7'*64),step_salt(self.request,0,'7'*64))
        self.assertNotEqual(step_salt(other,0,'7'*64),step_salt(other,0,'8'*64))
        self.request=other
        with patch.object(self.hosted,'evaluate',return_value=good):
            with self.assertRaises(Refused):self.evaluate()
        self.no_publication()
    def test_native_tools_cannot_downgrade_hosted_challenge(self):
        with self.assertRaises(Refused):self.service.evaluate(self.root,self.request,self.wasm)
        self.no_publication()
    def test_no_hosted_configuration_refuses(self):
        service=GateService(self.store,self.bk,self.gk,self.service.tools)
        with self.assertRaises(Refused):service.evaluate_hosted(self.root,self.request,self.wasm)
        self.no_publication()

if __name__=='__main__':unittest.main()
