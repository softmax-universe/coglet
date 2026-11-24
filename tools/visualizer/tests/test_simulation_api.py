import json
import threading
from urllib.request import Request, urlopen
from urllib.error import HTTPError

import pytest
from test_stream import load
from test_simulation import archive


def test_private_simulation_api_reexecutes_m2_and_never_exports_guest_code(tmp_path):
    server = load().make_server(tmp_path / 'empty')
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url, token = server.url.split('/#')
    source = archive()
    source['records'][0]['output_files']['/code/private'] = '736563726574'
    def request(headers):
        return Request(url + '/api/simulate', data=json.dumps({'archive': source}).encode(),
                       headers=headers, method='POST')
    try:
        with pytest.raises(HTTPError) as denied:
            urlopen(request({'Content-Type': 'application/json'}))
        assert denied.value.code == 403
        with urlopen(request({'Content-Type': 'application/json', 'Authorization': 'Bearer ' + token})) as response:
            result = json.load(response)
        assert result['simulation']['first_divergence'] is None
        assert '/code' not in json.dumps(result)
        assert result['records'][-1]['output_hash'] == source['records'][-1]['output_hash']
        with pytest.raises(HTTPError) as denied:
            urlopen(request({'Authorization': 'Bearer ' + token, 'Origin': 'https://evil.example'}))
        assert denied.value.code == 403
        duplicate = b'{"archive":null,"archive":' + json.dumps(source).encode() + b'}'
        with pytest.raises(HTTPError) as invalid:
            urlopen(Request(url + '/api/simulate', data=duplicate,
                            headers={'Authorization': 'Bearer ' + token}, method='POST'))
        assert invalid.value.code == 400
        with pytest.raises(HTTPError) as invalid:
            urlopen(Request(url + '/api/simulate', data=b'{}',
                            headers={'Authorization': 'Bearer ' + token, 'Content-Length': str(16 * 1024 * 1024 + 1)}, method='POST'))
        assert invalid.value.code == 400
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
