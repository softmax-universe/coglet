#!/usr/bin/env python3
"""Explicit native checks; missing proof binary fails, never a qualification skip.

Run: python3 tests/unforged_proof/native_replay_checks.py --driver PATH
This is local oracle coverage, NOT hosted stage-2 qualification.
"""
import argparse
import gzip
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
DRIVER = None


class NativeReplayChecks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.trace = Path(self.temp.name) / 'recorded.json'
        self.trace.write_bytes(gzip.decompress((ROOT / 'tools/visualizer/native/fixtures/m2-gridbot-trace.json.gz').read_bytes()))

    def invoke(self, *args, data=None):
        return subprocess.run([str(DRIVER), *map(str, args)], input=data, capture_output=True, timeout=120)

    def test_recorded_effectful_steps_are_reexecuted(self):
        result = self.invoke('record', self.trace)
        self.assertEqual(result.returncode, 0, result.stderr)
        recorded = json.loads(self.trace.read_bytes())
        actual = json.loads(result.stdout)
        self.assertEqual([r['output_hash'] for r in actual['records']], [r['output_hash'] for r in recorded['records']])
        self.assertGreater(sum(len(r['actions']) for r in actual['records']), 0)
        self.assertGreater(sum(len(r['boundary']) for r in actual['records']), 0)

    def test_fork_produces_complete_independently_replayable_archive(self):
        request = json.dumps({'receipt_index': 0, 'result': {'what_if': True}}).encode()
        result = self.invoke('fork-trace', self.trace, data=request)
        self.assertEqual(result.returncode, 0, result.stderr)
        original = json.loads(self.trace.read_bytes())
        fork = json.loads(result.stdout)
        self.assertNotEqual(fork['final_hash'], original['final_hash'])
        self.assertEqual(len(fork['records']), len(original['records']))
        self.trace.write_bytes(result.stdout)
        replay = self.invoke('record', self.trace)
        self.assertEqual(replay.returncode, 0, replay.stderr)
        self.assertEqual(json.loads(replay.stdout)['final_hash'], fork['final_hash'])
        for a, b in zip(original['records'], fork['records']):
            for key in ('actions', 'llm_requests', 'timers', 'status', 'failure'):
                self.assertEqual(a.get(key), b.get(key))

    def test_duplicate_and_nonregular_archives_refuse(self):
        result = self.invoke('fork-trace', self.trace, data=b'{"receipt_index":0,"result":{"x":1,"x":2}}')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b'duplicate JSON key', result.stderr)
        self.trace.write_text('{"records":[],"records":[]}')
        result = self.invoke('record', self.trace)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b'duplicate JSON key', result.stderr)
        result = self.invoke('record', self.trace.parent)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b'regular file', result.stderr)

    def test_absent_fork_receipt_fails_closed(self):
        result = self.invoke('fork-trace', self.trace, data=b'{"receipt_index":999,"result":{}}')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b'receipt index not found', result.stderr)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--driver', required=True, type=Path)
    options = parser.parse_args()
    DRIVER = options.driver.resolve()
    if not DRIVER.is_file():
        parser.error('build native proof driver first')
    unittest.main(argv=['native_replay_checks.py'])
