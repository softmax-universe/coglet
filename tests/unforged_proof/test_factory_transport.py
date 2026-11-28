"""Operator transport refusal tests; no remote execution is fabricated."""
from dataclasses import replace
import gzip
import json
from pathlib import Path
import tempfile
import unittest
import sys
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT/'crates/coglet-gate/python'))
from coglet_gate import ArtifactStore, TrustKey, Refused, digest
from factory_transport import FactoryTransport
from hosted_export import ExportAuthority
from guarded_transport import RecipeAuthority

ROOT=Path(__file__).resolve().parents[2]
class FactoryTransportTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name)
        self.trace=json.loads(gzip.decompress((ROOT/'tools/visualizer/native/fixtures/m2-gridbot-trace.json.gz').read_bytes()))
        self.request={'binding':{'coglet':self.trace['coglet']},'nonce':'1'*64}
        self.driver=self.root/'driver';self.driver.write_bytes(b'\x7fELFunit-fake')
        self.bridge=self.root/'bridge';self.bridge.write_bytes(b'never run')
        store=ArtifactStore(self.root/'authority')
        self.sa=ExportAuthority(store,TrustKey('source',b's'*32),set())
        self.ra=RecipeAuthority(store,TrustKey('wire',b'w'*32),set())
        self.transport=FactoryTransport(self.driver,self.bridge,digest(self.driver.read_bytes()),digest(self.bridge.read_bytes()),
            b'worker','a'*64,'native@sha256:'+'b'*64,'c'*64,self.root/'evidence',
            self.sa,self.ra,self.sa.issue,self.ra.issue,'explicit-independent-unit-operator',True)
    def test_no_synthetic_authorization_or_changed_binary_refuses(self):
        with self.assertRaises(Refused):replace(self.transport,synthetic_transport_only=False).identity
        self.bridge.write_bytes(b'changed')
        with self.assertRaises(Refused):self.transport.identity
    def test_denied_source_grant_prevents_preparation_and_bridge(self):
        with patch.object(ExportAuthority,'verify',side_effect=Refused('denied')),patch('guarded_preparation.state_files',side_effect=AssertionError('serialized')):
            with self.assertRaises(Refused):self.transport.evaluate(self.root,self.request,self.trace,attempt='d'*64)
        self.assertFalse((self.root/'evidence').exists())
        self.assertFalse((self.root/'step-0').exists())
    def test_denied_recipe_grant_prevents_staging_and_bridge(self):
        with patch.object(RecipeAuthority,'verify',side_effect=Refused('denied')),patch('guarded_transport.run_guarded',side_effect=AssertionError('started')):
            with self.assertRaises(Refused):self.transport.evaluate(self.root,self.request,self.trace,attempt='d'*64)
        self.assertFalse((self.root/'evidence').exists())
        self.assertFalse((self.root/'step-0').exists())

if __name__=='__main__':unittest.main()
