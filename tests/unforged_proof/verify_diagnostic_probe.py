#!/usr/bin/env python3
"""Independently verify captured sidecar bytes; never certify no live effects."""
import argparse
import hashlib
import json
from pathlib import Path
from diagnostic_receipts import verify_sidecar, verify_isolation_sidecar
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
    parser.add_argument('--strict-isolation', action='store_true')
    args = parser.parse_args()
    def events(path):
        return [json.loads(line, object_pairs_hook=unique)
                for line in (path/'bridge.jsonl').read_text().splitlines()]
    probe, readback = events(args.probe), events(args.readback)
    phases = ['submitted', 'executed', 'status', 'sidecar']
    if args.strict_isolation:
        phases.append('sandbox-output')
    if [event['phase'] for event in probe] != phases:
        raise Refused('ordered complete diagnostic execution required')
    submit, executed, status, sidecar = probe[:4]
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
    verify = verify_isolation_sidecar if args.strict_isolation else verify_sidecar
    observations = verify(raw[0]['raw_payload'].encode(), raw[0]['oid'],
                                  request=req, salt=salt)
    if observations != sidecar['observations']:
        raise Refused('captured parsed observations differ from exact raw object')
    if sidecar['raw_payload'] != raw[0]['raw_payload']:
        raise Refused('captured raw sidecar differs from readback')
    if args.strict_isolation:
        output = [event for event in readback if event['phase'] == 'sandbox-output-readback']
        if len(output) != 1 or output[0]['oid'] != executed['result_oid']:
            raise Refused('unique sandbox result readback required')
        payload = output[0]['raw_payload'].encode()
        oid = hashlib.sha1(b'blob '+str(len(payload)).encode()+b'\0'+payload).hexdigest()
        if (oid != output[0]['oid'] or probe[4]['oid'] != oid
                or probe[4]['raw_payload'] != output[0]['raw_payload']
                or output[0]['raw_payload'] != f"{observations['inner_netns']}\nunset\nclosed\n"):
            raise Refused('actual sandbox output differs from sidecar or hashed readback')
    print(json.dumps(dict(request=req, sidecar=raw[0]['oid'], observations=observations,
                          qualification='actual diagnostic transport ONLY; NOT stage2 PASS'), indent=2))


if __name__ == '__main__':
    main()
