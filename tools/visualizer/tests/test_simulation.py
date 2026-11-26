"""Replay actual M2 observations with the production world, not archived outputs."""
import copy
import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).parents[1]
FIXTURES = Path(__file__).parent / 'fixtures'


def load_sim():
    spec = importlib.util.spec_from_file_location('simulation', ROOT / 'simulation.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_edited_input_reports_first_divergence_without_mutating_archive():
    source = archive()
    original = copy.deepcopy(source)
    result = load_sim().replay(source, ['north', 'east', 'south', 'south'])
    assert result['first_divergence'] == 1
    assert result['frames'][-1]['hash'] != result['frames'][-1]['expected_hash']
    assert source == original


@pytest.mark.parametrize('field,value,reason', [
    ('seed', 43, 'replay mismatch'),
    ('actions', ['west'] * 4, 'replay mismatch'),
    ('records', [], 'observation count'),
    ('config', {'width': 8}, 'configuration'),
    ('actions', ['east'] * 1001, 'count limit'),
    ('seed', True, 'seed'),
])
def test_invalid_or_incomplete_archive_fails_closed(field, value, reason):
    source = archive()
    source[field] = value
    with pytest.raises(ValueError, match=reason):
        load_sim().replay(source)


@pytest.mark.parametrize('actions', [[], ['east'] * 3, ['<script>'] * 4, 'east'])
def test_fork_only_accepts_bounded_world_actions(actions):
    with pytest.raises(ValueError):
        load_sim().replay(archive(), actions)


def archive():
    trace = json.loads((FIXTURES / 'world-m2-gridbot-trace.json').read_text())
    provenance = json.loads((FIXTURES / 'world-m2-gridbot-trace.provenance.json').read_text())
    return dict(format='coglet-world-replay-v1', world=provenance['world'], seed=provenance['seed'],
                config=provenance['config'], actions=provenance['actions'],
                records=trace['records'])


def test_recording_with_different_world_configuration_is_refused_not_rewritten():
    # The fixture is recaptured under ADR-0035 with max_steps=16. A different
    # configuration must still be refused, never adapted by rewriting hashes.
    trace = json.loads((FIXTURES / 'native-m2-gridbot-trace.json').read_text())
    legacy = dict(archive(), records=trace['records'], config=dict(width=8, height=8, max_steps=10000))
    with pytest.raises(ValueError, match='replay mismatch'):
        load_sim().replay(legacy)
    with pytest.raises(ValueError, match='configuration'):
        load_sim().replay(dict(legacy, config=dict(width=8, height=8)))


def test_prepare_archive_cli_preserves_actual_records_and_refuses_overwrite(tmp_path):
    target = tmp_path / 'prepared.json'
    command = [sys.executable, str(ROOT / 'prepare_simulation.py'),
               '--trace', str(FIXTURES / 'world-m2-gridbot-trace.json'),
               '--world', 'gridbot-v1', '--seed', '42', '--config', '{"width":8,"height":8,"max_steps":16}',
               '--actions', '["east","east","south","south"]', '--output', str(target)]
    completed = subprocess.run(command, capture_output=True, text=True)
    assert completed.returncode == 0, completed.stderr
    assert json.loads(target.read_text()) == archive()
    assert target.stat().st_mode & 0o777 == 0o600
    assert subprocess.run(command, capture_output=True).returncode != 0
    assert json.loads(target.read_text()) == archive()


def test_recorded_m2_reexecutes_to_every_observed_world_hash():
    result = load_sim().replay(archive())
    assert result['verification'] == 'world-model-only'
    assert result['first_divergence'] is None
    assert len(result['frames']) == 5
    assert all(f['hash'] == f['expected_hash'] for f in result['frames'])
    assert result['frames'][-1]['state']['done'] is True
    observed = [e['payload']['state_hash'] for r in archive()['records']
                for e in r['events'] if e['kind'] == 'sim.gridbot_state']
    assert result['frames'][-1]['hash'] == observed[-1]
