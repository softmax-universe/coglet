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

    def test_native_model_request_and_recorded_reply_reexecute(self):
        result = self.invoke('record-model', ROOT)
        self.assertEqual(result.returncode, 0, result.stderr)
        recorded = json.loads(result.stdout)
        self.assertEqual(len(recorded['records']), 2)
        self.assertEqual(sum(len(r['llm_requests']) for r in recorded['records']), 1)
        self.assertEqual(recorded['records'][0]['status'], 'committed')
        self.assertEqual(recorded['records'][1]['status'], 'committed')
        self.assertIn(b'llm.reply', bytes.fromhex(recorded['records'][1]['boundary'][0]['after']))
        self.trace.write_bytes(result.stdout)
        replay = self.invoke('record', self.trace)
        self.assertEqual(replay.returncode, 0, replay.stderr)
        self.assertEqual([r['output_hash'] for r in json.loads(replay.stdout)['records']],
                         [r['output_hash'] for r in recorded['records']])
        from recorded_closure import require_model_reply_closure
        self.assertEqual(require_model_reply_closure(recorded), dict(requests=1, replies=1))
        # A genuine committed prefix with an outstanding request is valid archive
        # history but must not be accepted as a complete hosted gate recording.
        from run import prefix
        incomplete = prefix(recorded, 1)
        with self.assertRaisesRegex(ValueError, 'missing recorded model reply'):
            require_model_reply_closure(incomplete)
        self.trace.write_text(json.dumps(incomplete))
        output = self.trace.parent / 'must-not-create'
        refused = subprocess.run([
            'python3', str(ROOT/'tests/unforged_proof/prepare_oracle_inputs.py'),
            '--trace', str(self.trace), '--driver', str(DRIVER),
            '--unforge', str(self.trace.parent/'missing-unforge'), '--output', str(output),
            '--resolver-store', str(self.trace.parent/'missing-store'),
            '--expected-resolver', 'a'*64], capture_output=True, timeout=120)
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn(b'missing recorded model reply', refused.stderr)
        self.assertFalse(output.exists())
        import os
        recording = self.trace.parent / 'recording'
        recording.mkdir()
        (recording/'trace.json').write_text(json.dumps(incomplete))
        hosted = subprocess.run([
            'python3', str(ROOT/'tests/unforged_proof/hosted_probe.py'),
            '--recording', str(recording), '--driver', str(DRIVER),
            '--unforge', str(self.trace.parent/'missing-unforge'),
            '--bridge', str(self.trace.parent/'missing-bridge'), '--output', str(output),
            '--expected-endpoint', 'a'*64, '--expected-image', 'native@sha256:'+'b'*64,
            '--expected-resolver', 'c'*64, '--synthetic-transport-only', '--synthetic-model-oracle'],
            env={**os.environ, 'CAOS_IROH_TICKET': 'deliberately-invalid-local-negative'},
            capture_output=True, timeout=120)
        self.assertNotEqual(hosted.returncode, 0)
        self.assertIn(b'missing recorded model reply', hosted.stderr)
        self.assertFalse(output.exists())

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

    def test_snapshot_ingress_reuses_strict_archive_reader(self):
        link = self.trace.parent / 'link.json'
        link.symlink_to(self.trace)
        result = self.invoke('snapshot', link)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, b'')
        oversized = self.trace.parent / 'oversized.json'
        with oversized.open('wb') as stream:
            stream.truncate(128 * 1024 * 1024 + 1)
        result = self.invoke('snapshot', oversized)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b'128 MiB', result.stderr)

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
