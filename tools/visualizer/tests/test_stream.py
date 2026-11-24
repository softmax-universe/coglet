"""A record stream is an explicitly selected operator export, not WAL scraping."""
import importlib.util
import json
import threading
from urllib.request import Request, urlopen
from urllib.error import HTTPError
import pytest
from pathlib import Path


def load():
    spec = importlib.util.spec_from_file_location("visualizer", Path(__file__).parents[1] / "server.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_partial_append_is_not_published(tmp_path):
    path = tmp_path / "records.jsonl"
    path.write_text('{"step":')
    store = load().RecordStream(path)
    assert store.snapshot()["records"] == []
    assert store.snapshot()["state"] == "waiting"


def record(step=0):
    return {"version": "0.1", "coglet": "coglet:" + "1" * 64, "trace_id": "5" * 32,
            "step": step, "logical_time": step, "status": "committed", "output_hash": "a" * 64,
            "record_hash": f"{step:064x}", "events": [], "output_files": {"/data/count": "30"}}


def test_projection_is_bounded_and_never_exports_code(tmp_path):
    path = tmp_path / "records"
    values = [record(i) for i in range(140)]
    values[0]["output_files"]["/code/private"] = "736563726574"
    path.write_text(''.join(json.dumps(item) + '\n' for item in values))
    data = load().RecordStream(path).snapshot()
    assert len(data["records"]) == 128
    assert data["records"][0]["step"] == 12
    assert data["omitted"] == 12
    assert data["modified_at"] > 0
    path.write_text(json.dumps(values[0]) + '\n')
    assert '/code/private' not in load().RecordStream(path).snapshot()["records"][0]["output_files"]


@pytest.mark.parametrize('value', [{}, {**record(), 'step': True}, {**record(), 'events': 'bad'},
                                  {**record(), 'output_files': {'/data/x': 'zz'}},
                                  {**record(), 'logical_time': float('nan')}])
def test_malformed_complete_record_fails_closed(tmp_path, value):
    path = tmp_path / 'records'
    path.write_text(json.dumps(value) + '\n')
    with pytest.raises(ValueError):
        load().RecordStream(path).snapshot()


def test_atomic_trace_export_uses_records_not_archive_internals(tmp_path):
    path = tmp_path / 'trace.json'
    path.write_text(json.dumps({'records': [record()], 'blobs': {'private': 'not exported'}}))
    stream = load().RecordStream(path, source_format='trace')
    assert stream.snapshot()['records'][0]['step'] == 0
    successor = tmp_path / 'next.json'
    successor.write_text(json.dumps({'records': [record(), record(1)]}))
    successor.replace(path)
    assert stream.snapshot()['records'][-1]['step'] == 1
    path.write_text('{')
    with pytest.raises(ValueError):
        stream.snapshot()


def test_no_symlinks_or_unbounded_file_reads(tmp_path):
    server = load()
    path = tmp_path / 'records'; path.write_text('')
    link = tmp_path / 'link'; link.symlink_to(path)
    with pytest.raises((ValueError, OSError)):
        server.RecordStream(link).snapshot()
    with path.open('wb') as output:
        output.truncate(server.MAX_BYTES + 1)
    with pytest.raises(ValueError, match='limit'):
        server.RecordStream(path).snapshot()


def test_http_capability_origin_host_and_headers(tmp_path):
    path = tmp_path / 'records'; path.write_text(json.dumps(record()) + '\n')
    server = load().make_server(path)
    thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
    base, token = server.url.split('#')
    def get(route, headers):
        return urlopen(Request(base + route, headers=headers), timeout=3)
    auth = {'Authorization': 'Bearer ' + token}
    try:
        for headers in [{}, {**auth, 'Origin': 'https://evil.example'}, {**auth, 'Host': 'evil.example'}]:
            with pytest.raises(HTTPError) as error:
                get('api/snapshot', headers)
            assert error.value.code == 403
        with get('api/snapshot', auth) as reply:
            assert reply.headers['Cache-Control'] == 'no-store'
            assert reply.headers['X-Content-Type-Options'] == 'nosniff'
            assert "frame-ancestors 'none'" in reply.headers['Content-Security-Policy']
            assert len(json.load(reply)['records']) == 1
        path.unlink()
        with get('api/snapshot', auth) as reply:
            data = json.load(reply)
            assert data['state'] == 'error' and data['records'] == []
    finally:
        server.shutdown(); server.server_close(); thread.join(timeout=5)
