"""Service refusal before starting any unpinned native executable."""
from pathlib import Path

import pytest
from coglet_gate import GateService, GateVerifier, Refused
from coglet_sim.evaluation import NativeTools, setup
from test_gate_artifacts import environment


def test_native_schedule_uses_exact_requested_seed():
    assert setup(b'wasm', 'gridbot', 42)['native_schedule']['seed'] == 42
    assert setup(b'wasm', 'cartpole', 2**64 - 1)['native_schedule']['seed'] == 2**64 - 1


def test_service_refuses_changed_tools_and_forged_challenge_before_execution(tmp_path):
    store, build_key, key, binding, suite, build, _ = environment(tmp_path)
    paths = []
    for name in ('daemon', 'proof'):
        path = tmp_path / name
        path.write_bytes(b'not a real executable; it must never run')
        path.chmod(0o700)
        paths.append(path)
    service = GateService(store, build_key, key, NativeTools(*paths))
    verifier = GateVerifier(tmp_path / 'service.sqlite', store, build_key, key,
                            tools=service.tool_identity)
    request = verifier.challenge_fixture(binding, suite, build)
    forged = {**request, 'nonce': 'e' * 64}
    with pytest.raises(Refused, match='authentication'):
        service.evaluate(tmp_path, forged, b'wasm')
    request['tools']['daemon'] = 'different'
    with pytest.raises(Refused, match='authentication'):
        service.evaluate(tmp_path, request, b'wasm')
    request = verifier.challenge_fixture(binding, suite, build)
    paths[0].write_bytes(b'changed binary')
    with pytest.raises(Refused, match='tool changed'):
        service.evaluate(tmp_path, request, b'wasm')
    # No evaluation is issued for infrastructure/refusal failures.
    evaluations = []
    for path in store.root.iterdir():
        import json
        if path.is_file() and json.loads(path.read_bytes())['kind'] == 'evaluation':
            evaluations.append(path)
    assert evaluations == []
