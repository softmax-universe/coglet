"""Native evaluation must use explicit artifacts, not test environment globals."""
from hashlib import sha256
import os
from pathlib import Path

import pytest
from coglet_sim.evaluation import NativeTools, evaluate

TOOLS = ('COGLET_SANDBOX_DAEMON', 'COGLET_SANDBOX_PROOF', 'COGLET_SANDBOX_WASM')


def test_optimized_interpreter_refuses_evaluation():
    import subprocess
    import sys
    result = subprocess.run([sys.executable, '-O', '-c',
        "from coglet_sim.evaluation import evaluate; evaluate(None, None, b'x', world='cartpole', seed=42)"],
        capture_output=True, text=True)
    assert result.returncode != 0
    assert 'optimized interpreter unsupported' in result.stderr


def test_native_tools_require_existing_executables(tmp_path):
    with pytest.raises(ValueError, match='executable'):
        NativeTools(tmp_path / 'absent', tmp_path / 'absent')


@pytest.mark.skipif(not all(os.environ.get(k) for k in TOOLS), reason='native tools required')
def test_evaluation_binds_exact_wasm_and_reports_failed_behavior(tmp_path):
    tools = NativeTools(Path(os.environ[TOOLS[0]]), Path(os.environ[TOOLS[1]]))
    wasm = Path(os.environ[TOOLS[2]]).read_bytes()
    result = evaluate(tmp_path, tools, wasm, world='cartpole', seed=42,
                      max_steps=16, minimum_actions=17)
    assert result['rewritten_sha256'] == sha256(wasm).hexdigest()
    assert result['world'] == 'cartpole' and result['seed'] == 42
    assert result['max_steps'] == 16 and result['minimum_actions'] == 17
    assert result['action_count'] == 16
    assert result['independent_guest_reexecution'] is True
    assert result['passed'] is False
    assert result['failure'] == 'minimum_actions'
