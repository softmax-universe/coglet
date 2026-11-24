"""Explicit native acceptance: build the helper first; missing tools fail closed."""
import gzip
import hashlib
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def test_real_m2_export_and_independent_native_replay(tmp_path):
    helper = Path(os.environ.get('COGLET_VISUALIZER_NATIVE', ROOT / 'target/debug/coglet-visualizer-native'))
    assert helper.is_file(), 'Build the native visualizer helper before native acceptance'
    compressed = (ROOT / 'fixtures/m2-gridbot-trace.json.gz').read_bytes()
    provenance = json.loads((ROOT / 'fixtures/provenance.json').read_text())
    assert hashlib.sha256(compressed).hexdigest() == provenance['fixture_sha256']
    raw = gzip.decompress(compressed)
    assert hashlib.sha256(raw).hexdigest() == provenance['canonical_trace_sha256']
    archive = tmp_path / 'trace.json'
    archive.write_bytes(raw)
    result = subprocess.run([str(helper), 'replay', str(archive)],
                            input='{}', text=True, capture_output=True, check=True)
    report = json.loads(result.stdout)
    trace = json.loads(archive.read_text())
    assert report['verified_native'] is True
    assert report['final_hash'] == trace['final_hash']
    assert report['hashes'] == [r['output_hash'] for r in trace['records']]
    assert len(report['records']) == 9
    assert bytes.fromhex(report['records'][-1]['output_files']['/data/inbox.jsonl']).count(b'\n') == 4
    assert all(not any(p.startswith('/code/') for p in r['output_files']) for r in report['records'])
    fork = subprocess.run([str(helper), 'replay', str(archive)],
                          input=json.dumps({'receipt_index': 0, 'result': {'what_if': True}}),
                          text=True, capture_output=True)
    assert fork.returncode == 0, fork.stderr
    edited = json.loads(fork.stdout)
    assert edited['baseline_final_hash'] == trace['final_hash']
    assert edited['final_hash'] != trace['final_hash']
    assert edited['first_divergence'] is not None
    assert len(edited['records']) == len(report['records'])
    inbox = bytes.fromhex(edited['records'][-1]['output_files']['/data/inbox.jsonl']).decode()
    messages = [json.loads(line) for line in inbox.splitlines()]
    assert len(messages) == 4
    assert messages[0]['event']['payload']['result'] == {'what_if': True}
    assert all(m['event']['payload']['result'] == {} for m in messages[1:])
    again = subprocess.run([str(helper), 'replay', str(archive)], input='{}',
                           text=True, capture_output=True, check=True)
    assert json.loads(again.stdout)['final_hash'] == trace['final_hash']
    duplicate = subprocess.run([str(helper), 'replay', str(archive)],
                               input='{"receipt_index":0,"result":{},"result":{"what_if":true}}',
                               text=True, capture_output=True)
    assert duplicate.returncode != 0
    assert 'duplicate JSON key' in duplicate.stderr
