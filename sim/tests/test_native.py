"""Real native acceptance, enabled by sim/check-native.sh."""
import os
from pathlib import Path

import pytest
from coglet_sim.evaluation import NativeTools, evaluate

TOOLS = ('COGLET_SANDBOX_DAEMON', 'COGLET_SANDBOX_PROOF', 'COGLET_SANDBOX_WASM')
pytestmark = pytest.mark.skipif(not all(os.environ.get(k) for k in TOOLS),
                              reason='native proof executes in Rust CI via sim/check-native.sh')


def episode(tmp_path, world):
    tools = NativeTools(Path(os.environ[TOOLS[0]]), Path(os.environ[TOOLS[1]]))
    report = evaluate(tmp_path, tools, Path(os.environ[TOOLS[2]]).read_bytes(),
                      world=world, seed=42, max_steps=16)
    assert report['passed'], report
    print(f"{world}: {report['action_count']} actions; "
          f"{len(report['runtime_state_hashes'])} live/replay guest hashes equal")


def test_gridbot_pilot_reaches_goal_live_and_replayed(tmp_path):
    episode(tmp_path, 'gridbot')


def test_cartpole_episode_live_and_replayed(tmp_path):
    episode(tmp_path, 'cartpole')
