#!/usr/bin/env python3
"""Independently verify captured sidecar bytes; never certify no live effects."""
import argparse
import json
from pathlib import Path
from diagnostic_receipts import verify_sidecar
from hosted_receipts import Refused


def unique(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise Refused('duplicate transcript field')
        value[key] = item
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--probe', type=Path, required=True)
    parser.add_argument('--readback', type=Path, required=True)
    parser.add_argument('--expected-image', required=True)
    args = parser.parse_args()
    def events(path):
        return [json.loads(line, object_pairs_hook=unique)
                for line in (path/'bridge.jsonl').read_text().splitlines()]
    probe, readback = events(args.probe), events(args.readback)
    if [event['phase'] for event in probe] != ['submitted', 'executed', 'status', 'sidecar']:
        raise Refused('ordered complete diagnostic execution required')
    submit, executed, status, sidecar = probe
    req, salt = submit['request'], submit['salt']
    if (executed['request'] != req or executed['cache_hit'] is not False
            or executed['server_confirmed_nocache'] is not True
            or executed['worker_image'] != args.expected_image
            or submit['expected_image'] != args.expected_image
            or status['request'] != req or status['http_status'] != 200
            or status['body']['arg_tree'] != req
            or status['body']['out_trace'] != [sidecar['oid']]
            or sidecar['request'] != req):
        raise Refused('diagnostic execution/status binding mismatch')
    complete = [event for event in readback
                if event.get('path') == f'/status/{req}?all=1']
    raw = [event for event in readback if event['phase'] == 'sidecar-readback']
    if len(complete) != 1 or len(raw) != 1:
        raise Refused('unique completed status and raw sidecar required')
    if (complete[0]['http_status'] != 200
            or json.loads(complete[0]['body'], object_pairs_hook=unique) != status['body']
            or raw[0]['request'] != req or raw[0]['oid'] != sidecar['oid']):
        raise Refused('independent status/sidecar readback mismatch')
    observations = verify_sidecar(raw[0]['raw_payload'].encode(), raw[0]['oid'],
                                  request=req, salt=salt)
    if observations != sidecar['observations']:
        raise Refused('captured parsed observations differ from exact raw object')
    print(json.dumps(dict(request=req, sidecar=raw[0]['oid'], observations=observations,
                          qualification='actual diagnostic transport ONLY; NOT stage2 PASS'), indent=2))


if __name__ == '__main__':
    main()
