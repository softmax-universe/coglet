"""Additional adversarial cases for local authenticated testbed evidence."""
from concurrent.futures import ThreadPoolExecutor
from copy import deepcopy
from hashlib import sha256
import json

import pytest
from coglet_gate import GateVerifier, Refused, TrustKey
from test_gate_artifacts import environment


def signed_fixture(store, gate_key, binding, suite, challenge):
    return store.put(dict(request=challenge, tools=challenge['tools'], qualification='fixture-only',
                          report={**{k: v for k, v in suite.document().items() if k != 'suite'},
                                  'rewritten_sha256': binding.rewritten_sha256,
                                  'coglet': binding.coglet,
                                  'independent_guest_reexecution': True, 'passed': True}),
                     gate_key, 'evaluation')


def test_recomputed_content_address_does_not_bypass_mac(tmp_path):
    store, _, key, binding, suite, build, verifier = environment(tmp_path)
    challenge = verifier.challenge_fixture(binding, suite, build)
    ref = signed_fixture(store, key, binding, suite, challenge)
    altered = json.loads((store.root / ref).read_bytes())
    altered['payload']['report']['passed'] = False
    raw = json.dumps(altered, sort_keys=True, separators=(',', ':')).encode()
    forged = sha256(raw).hexdigest()
    (store.root / forged).write_bytes(raw)
    with pytest.raises(Refused):
        verifier.accept_fixture(challenge, forged)
    assert verifier.accept_fixture(challenge, ref)['report']['passed']


@pytest.mark.parametrize('field,value', [('world', 'cartpole'), ('seed', 43),
    ('max_steps', 18), ('minimum_actions', 4), ('rewritten_sha256', 'f' * 64),
    ('independent_guest_reexecution', False), ('passed', 1)])
def test_even_authenticated_mismatched_report_is_refused(tmp_path, field, value):
    store, _, key, binding, suite, build, verifier = environment(tmp_path)
    challenge = verifier.challenge_fixture(binding, suite, build)
    ref = signed_fixture(store, key, binding, suite, challenge)
    payload = store.read(ref, key, 'evaluation')
    payload['report'][field] = value
    mismatched = store.put(payload, key, 'evaluation')
    with pytest.raises(Refused):
        verifier.accept_fixture(challenge, mismatched)
    assert verifier.accept_fixture(challenge, ref)['report']['passed']


def test_changed_tool_and_missing_operator_pin_are_refused(tmp_path):
    store, build_key, gate_key, binding, suite, build, verifier = environment(tmp_path)
    challenge = verifier.challenge_fixture(binding, suite, build)
    ref = signed_fixture(store, gate_key, binding, suite, challenge)
    unknown = GateVerifier(tmp_path / 'unknown.sqlite', store, build_key, gate_key)
    with pytest.raises(Refused, match='pin'):
        unknown.challenge_fixture(binding, suite, build)
    payload = store.read(ref, gate_key, 'evaluation')
    payload['tools']['daemon'] = 'different'
    wrong = store.put(payload, gate_key, 'evaluation')
    with pytest.raises(Refused):
        verifier.accept_fixture(challenge, wrong)
    # Mutating returned request cannot change the verifier's pinned expectations.
    challenge['tools']['daemon'] = 'different'
    assert verifier.tools == {'daemon': 'd', 'proof': 'p'}
    with pytest.raises(Refused):
        verifier.accept_fixture(challenge, wrong)
    with pytest.raises(Refused):
        GateVerifier(tmp_path / 'bad-key.sqlite', store, build_key,
                     TrustKey('other-id', build_key.secret))


def test_concurrent_consumers_exactly_one_and_unknown_nonce_refused(tmp_path):
    store, build_key, key, binding, suite, build, verifier = environment(tmp_path)
    challenge = verifier.challenge_fixture(binding, suite, build)
    ref = signed_fixture(store, key, binding, suite, challenge)
    # Different DB has no admission custody even with correct keys/artifacts.
    unknown = GateVerifier(tmp_path / 'other.sqlite', store, build_key, key, tools=verifier.tools)
    with pytest.raises(Refused, match='unknown'):
        unknown.accept_fixture(challenge, ref)

    def consume(_):
        other = GateVerifier(tmp_path / 'challenges.sqlite', store, build_key, key, tools=verifier.tools)
        try:
            other.accept_fixture(deepcopy(challenge), ref)
            return 'accepted'
        except Refused:
            return 'refused'

    with ThreadPoolExecutor(max_workers=2) as pool:
        outcomes = list(pool.map(consume, range(2)))
    assert sorted(outcomes) == ['accepted', 'refused']
