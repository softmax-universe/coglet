import gzip
import json
import os
from pathlib import Path
import subprocess

import pytest

ROOT = Path(__file__).resolve().parents[1]


def helper():
    executable = Path(os.environ.get('COGLET_VISUALIZER_NATIVE', ROOT / 'target/debug/coglet-visualizer-native'))
    assert executable.is_file(), 'Build the native helper first'
    return str(executable)


def test_native_archive_symlink_is_refused(tmp_path):
    source = tmp_path / 'trace.json'
    source.write_bytes(gzip.decompress((ROOT / 'fixtures/m2-gridbot-trace.json.gz').read_bytes()))
    link = tmp_path / 'link.json'
    link.symlink_to(source)
    result = subprocess.run([helper(), 'replay', str(link)], input='{}', text=True, capture_output=True)
    assert result.returncode != 0
    assert not result.stdout


@pytest.mark.parametrize('field', ['initial_actions', 'initial_effect_ids', 'initial_inbox_wakes', 'initial_llm_requests', 'initial_timers', 'blobs'])
def test_incomplete_archive_never_qualifies_native_replay(tmp_path, field):
    trace = json.loads(gzip.decompress((ROOT / 'fixtures/m2-gridbot-trace.json.gz').read_bytes()))
    del trace[field]
    source = tmp_path / 'incomplete.json'
    source.write_text(json.dumps(trace))
    result = subprocess.run([helper(), 'replay', str(source)], input='{}', text=True, capture_output=True)
    assert result.returncode != 0
    assert not result.stdout


def test_edited_input_out_of_range_does_not_return_previous_result(tmp_path):
    source = tmp_path / 'trace.json'
    source.write_bytes(gzip.decompress((ROOT / 'fixtures/m2-gridbot-trace.json.gz').read_bytes()))
    result = subprocess.run([helper(), 'replay', str(source)],
                            input='{"receipt_index":999,"result":{"what_if":true}}', text=True, capture_output=True)
    assert result.returncode != 0
    assert 'receipt index not found' in result.stderr
    assert not result.stdout
