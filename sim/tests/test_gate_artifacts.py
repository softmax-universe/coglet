"""Trust-boundary tests; fixture signing never qualifies production builds."""
from dataclasses import replace
from hashlib import sha256
import json
import os
from pathlib import Path

import pytest
from coglet_gate import (ArtifactStore, Binding, BuildIssuer, GateService, GateVerifier,
                         Refused, Suite, TrustKey)
from coglet_sim.evaluation import NativeTools


def environment(tmp_path):
    store = ArtifactStore(tmp_path / 'artifacts')
    build_key = TrustKey('fixture-build', b'b' * 32)
    gate_key = TrustKey('sandbox-gate', b'g' * 32)
    binding = Binding('coglet:' + 'b' * 64, 'test-repo', 'a' * 40, 'b' * 40,
                      'c' * 32, sha256(b'wasm').hexdigest())
    suite = Suite('gridbot-v1', 'gridbot', 42, 16, 1)
    build = BuildIssuer(store, build_key).fixture(binding, b'wasm', b'fixture-provenance')
    verifier = GateVerifier(tmp_path / 'challenges.sqlite', store, build_key, gate_key,
                            tools={'daemon': 'd', 'proof': 'p'})
    return store, build_key, gate_key, binding, suite, build, verifier


def test_artifacts_are_content_addressed_and_tamper_refused(tmp_path):
    store, key, _, binding, _, build, _ = environment(tmp_path)
    payload = store.read(build, key, 'build')
    assert payload['binding'] == binding.document()
    assert payload['qualification'] == 'fixture-only'
    assert store.put(payload, key, 'build') == build
    path = store.root / build
    raw = json.loads(path.read_bytes())
    raw['payload']['qualification'] = 'production'
    path.chmod(0o600)  # simulate filesystem tampering, not an API write
    path.write_text(json.dumps(raw))
    with pytest.raises(Refused):
        store.read(build, key, 'build')


def test_issuer_refuses_wrong_bytes_and_fixture_never_becomes_production(tmp_path):
    store, key, _, binding, suite, build, verifier = environment(tmp_path)
    with pytest.raises(Refused):
        BuildIssuer(store, key).fixture(binding, b'different', b'fixture')
    with pytest.raises(Refused, match='production'):
        verifier.verify_production(binding, suite, build)
    # Re-signing a caller-controlled qualification label is insufficient too.
    payload = store.read(build, key, 'build')
    payload['qualification'] = 'production'
    forged = store.put(payload, key, 'build')
    with pytest.raises(Refused):
        verifier.challenge_fixture(binding, suite, forged)
    with pytest.raises(Refused, match='production'):
        verifier.verify_production(binding, suite, forged)


@pytest.mark.parametrize('field,value', [
    ('coglet', 'coglet:' + 'a' * 64), ('repo', 'another-repo'),
    ('candidate', 'd' * 40), ('base', None), ('revision', 'e' * 32),
    ('rewritten_sha256', 'f' * 64),
])
def test_build_exact_binding_refuses_transfer(tmp_path, field, value):
    _, _, _, binding, suite, build, verifier = environment(tmp_path)
    with pytest.raises(Refused):
        verifier.challenge_fixture(replace(binding, **{field: value}), suite, build)


def test_verifier_authentication_challenge_binding_and_restart_replay(tmp_path):
    store, build_key, gate_key, binding, suite, build, verifier = environment(tmp_path)
    challenge = verifier.challenge_fixture(binding, suite, build)
    # Synthetic report exists only in this unit test; native service has no sign-result API.
    result = dict(request=challenge, tools={'daemon': 'd', 'proof': 'p'},
                  report={**{k: v for k, v in suite.document().items() if k != 'suite'},
                          'rewritten_sha256': binding.rewritten_sha256,
                          'independent_guest_reexecution': True, 'passed': True},
                  qualification='fixture-only')
    artifact = store.put(result, gate_key, 'evaluation')
    with pytest.raises(Refused):
        verifier.verify_production(challenge, artifact)
    other = verifier.challenge_fixture(binding, replace(suite, seed=43), build)
    with pytest.raises(Refused):
        verifier.accept_fixture(other, artifact)
    for field, value in [('suite', 'other'), ('seed', 4), ('max_steps', 17),
                         ('minimum_actions', 2), ('world', 'cartpole')]:
        changed = dict(challenge)
        changed['suite'] = {**challenge['suite'], field: value}
        with pytest.raises(Refused):
            verifier.accept_fixture(changed, artifact)
    with pytest.raises(Refused):
        store.read(artifact, TrustKey('sandbox-gate', b'x' * 32), 'evaluation')
    with pytest.raises(Refused):
        store.read(artifact, gate_key, 'build')
    assert verifier.accept_fixture(challenge, artifact)['report']['passed']
    restarted = GateVerifier(tmp_path / 'challenges.sqlite', store, build_key, gate_key,
                             tools={'daemon': 'd', 'proof': 'p'})
    with pytest.raises(Refused, match='replay'):
        restarted.accept_fixture(challenge, artifact)
    with pytest.raises(Refused, match='replay'):
        restarted.accept_fixture(challenge, artifact)


def test_invalid_bindings_and_suite_fail_closed(tmp_path):
    _, _, _, binding, suite, _, _ = environment(tmp_path)
    for field, value in [('candidate', 'short'), ('base', 'b' * 64),
                         ('revision', 'r1'), ('rewritten_sha256', 'F' * 64)]:
        with pytest.raises(Refused):
            replace(binding, **{field: value})
    for field, value in [('seed', True), ('seed', -1), ('max_steps', 0),
                         ('minimum_actions', 0), ('world', 'unknown')]:
        with pytest.raises(Refused):
            replace(suite, **{field: value})


TOOLS = ('COGLET_SANDBOX_DAEMON', 'COGLET_SANDBOX_PROOF', 'COGLET_SANDBOX_WASM')
@pytest.mark.skipif(not all(os.environ.get(k) for k in TOOLS), reason='run sim/check-native.sh')
@pytest.mark.parametrize('world', ['gridbot', 'cartpole'])
@pytest.mark.parametrize('passed', [True, False], ids=['pass', 'reject'])
def test_native_authenticated_gridbot_cartpole_pass_and_reject(tmp_path, world, passed):
    store, build_key, gate_key, binding, _, _, verifier = environment(tmp_path)
    wasm = Path(os.environ[TOOLS[2]]).read_bytes()
    binding = replace(binding, rewritten_sha256=sha256(wasm).hexdigest())
    build = BuildIssuer(store, build_key).fixture(binding, wasm, b'M2 fixture, not production')
    steps = 16 if passed else (1 if world == 'gridbot' else 128)
    suite = Suite(world + '-v1', world, 42, steps, 1)
    tools = NativeTools(Path(os.environ[TOOLS[0]]), Path(os.environ[TOOLS[1]]))
    service = GateService(store, build_key, gate_key, tools)
    verifier = GateVerifier(tmp_path / 'native.sqlite', store, build_key, gate_key,
                            tools=service.tool_identity)
    challenge = verifier.challenge_fixture(binding, suite, build)
    artifact = service.evaluate(tmp_path, challenge, wasm)
    report = verifier.accept_fixture(challenge, artifact)
    assert report['report']['passed'] is passed
    if not passed:
        assert report['report']['failure'] == ('goal_not_reached' if world == 'gridbot'
                                                else 'episode_terminated_early')
    assert report['report']['independent_guest_reexecution']
    assert report['report']['rewritten_sha256'] == binding.rewritten_sha256
    assert report['request']['suite'] == suite.document()
    assert report['tools'] == service.tool_identity
    with pytest.raises(Refused, match='replay'):
        verifier.accept_fixture(challenge, artifact)
    with pytest.raises(Refused):
        service.evaluate(tmp_path, challenge, wasm + b'\0')
