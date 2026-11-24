"""Real native replay through the private loopback facade."""
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import threading
from urllib.request import Request, urlopen
from urllib.error import HTTPError

import pytest

ROOT = Path(__file__).resolve().parents[1]


def load_server():
    spec = importlib.util.spec_from_file_location('native_http_server', ROOT.parent / 'server.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def native_source(tmp_path):
    compressed = (ROOT / 'fixtures/m2-gridbot-trace.json.gz').read_bytes()
    provenance = json.loads((ROOT / 'fixtures/provenance.json').read_text())
    assert hashlib.sha256(compressed).hexdigest() == provenance['fixture_sha256']
    raw = gzip.decompress(compressed)
    assert hashlib.sha256(raw).hexdigest() == provenance['canonical_trace_sha256']
    archive = tmp_path / 'trace.json'
    archive.write_bytes(raw)
    helper = Path(os.environ['COGLET_VISUALIZER_NATIVE'])
    assert helper.is_file()
    return archive, helper, json.loads(raw)


def test_private_native_api_replays_forks_and_refuses_replaced_source(native_source, tmp_path):
    archive, helper, trace = native_source
    server = load_server().make_server(tmp_path / 'empty', native_trace=archive, native_helper=helper)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url, token = server.url.split('/#')

    def request(raw=b'{}', **headers):
        return urlopen(Request(url + '/api/native', data=raw, method='POST',
                              headers={'Authorization': 'Bearer ' + token, **headers}), timeout=35)
    try:
        with pytest.raises(HTTPError) as denied:
            request(Authorization='Bearer invalid')
        assert denied.value.code == 403
        with request() as response:
            result = json.load(response)
        assert result['native']['verified_native'] is True
        assert result['native']['final_hash'] == trace['final_hash']
        assert result['records'][-1]['output_hash'] == trace['final_hash']
        assert '/code/' not in json.dumps(result)
        with request(b'{"receipt_index":0,"result":{"what_if":true}}') as response:
            fork = json.load(response)
        assert fork['native']['final_hash'] != trace['final_hash']
        assert fork['native']['baseline_final_hash'] == trace['final_hash']
        assert fork['native']['first_divergence'] is not None
        messages = [json.loads(line) for line in bytes.fromhex(fork['records'][-1]['output_files']['/data/inbox.jsonl']).decode().splitlines()]
        assert len(messages) == 4
        assert messages[0]['event']['payload']['result'] == {'what_if': True}
        for raw in [b'{"path":"/etc/passwd"}', b'{"result":{},"result":{}}', b'{', b'x' * 65537]:
            with pytest.raises(HTTPError) as invalid:
                request(raw)
            assert invalid.value.code == 400
        with request() as response:
            assert json.load(response)['native']['final_hash'] == trace['final_hash']
        old = archive.with_suffix('.old')
        archive.rename(old)
        archive.write_bytes(old.read_bytes())
        with pytest.raises(HTTPError) as stale:
            request()
        assert stale.value.code == 400
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
