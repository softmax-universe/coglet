"""CLI boundary regressions; not hosted execution evidence."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('prepared_probe.py')

class PreparedProbeTests(unittest.TestCase):
    def invoke(self,trace,extra):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root/'trace').write_bytes(trace)
            (root/'snapshot').write_bytes(b'{}')
            (root/'driver').write_bytes(b'\x7fELFtest')
            command = [sys.executable,str(SCRIPT),'--trace',str(root/'trace'),'--snapshot',str(root/'snapshot'),
                '--driver',str(root/'driver'),'--bridge',str(root/'absent-bridge'),'--output',str(root/'output'),
                '--oracle','model','--step','0','--coglet','coglet:'+'a'*64,
                '--expected-endpoint','b'*64,'--expected-image','native@sha256:'+'c'*64,
                '--expected-bridge-sha256','d'*64,*extra]
            result = subprocess.run(command,capture_output=True,timeout=20)
            self.assertFalse((root/'output').exists())
            return result

    def test_initial_denial_precedes_json_and_native_process(self):
        result = self.invoke(b'not json',['--synthetic-transport-only','--deny-export'])
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout),dict(denied=True,native_started=False,bridge_started=False,private_staging_writes=0))

    def test_synthetic_ack_required(self):
        result = self.invoke(b'not json',[])
        self.assertNotEqual(result.returncode,0)
        self.assertIn(b'synthetic-only prepared export',result.stderr)

    def test_missing_reply_refuses_before_native_and_bridge(self):
        trace = dict(initial_llm_requests=[dict(id='a',correlation_id='c')],records=[],trailing_boundary=None)
        result = self.invoke(json.dumps(trace).encode(),['--synthetic-transport-only'])
        self.assertNotEqual(result.returncode,0)
        self.assertIn(b'missing recorded model reply',result.stderr)
        self.assertNotIn(b'Exec format',result.stderr)

if __name__ == '__main__': unittest.main()
