"""Verify actual diagnostic sidecar bytes and invocation, not effective isolation.

The caller must obtain the OID from the authenticated completed status of a fresh
request whose execution receipt/image it has independently verified. This helper
alone does not authenticate status, prove an execution, or qualify a gate.
"""
import hashlib
import json
import re

from hosted_receipts import Refused


def _decode_sidecar(raw, oid, *, request, salt):
    if (not isinstance(raw, bytes) or len(raw) > 65536
            or not isinstance(oid, str) or not re.fullmatch('[0-9a-f]{40}', oid)
            or not isinstance(request, str) or not re.fullmatch('[0-9a-f]{40}', request)
            or not isinstance(salt, str) or not re.fullmatch('[0-9a-f]{64}', salt)):
        raise Refused('bounded raw sidecar and exact invocation identities required')
    actual = hashlib.sha1(b'blob '+str(len(raw)).encode()+b'\0'+raw).hexdigest()
    if actual != oid:
        raise Refused('diagnostic raw Git blob identity mismatch')
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise Refused('duplicate diagnostic observation field')
            result[key] = value
        return result
    try:
        value = json.loads(raw, object_pairs_hook=unique)
    except (ValueError, UnicodeError) as error:
        raise Refused('invalid diagnostic JSON') from error
    if (not isinstance(value, dict) or value.get('request') != request
            or value.get('salt') != salt):
        raise Refused('diagnostic invocation mismatch')
    return value


def verify_sidecar(raw, oid, *, request, salt):
    value = _decode_sidecar(raw, oid, request=request, salt=salt)
    if (set(value) != {'request', 'salt', 'netns', 'sandbox_env', 'network_env'}
            or any(not isinstance(item, str) for item in value.values())):
        raise Refused('closed diagnostic observations or invocation mismatch')
    return value


def verify_isolation_sidecar(raw, oid, *, request, salt):
    """Strict nested-namespace observation, NOT a complete no-effects certificate."""
    value = _decode_sidecar(raw, oid, request=request, salt=salt)
    if (set(value) != {'request', 'salt', 'outer_netns', 'inner_netns',
                       'exit_code', 'proxy_env', 'fd3'}
            or type(value['exit_code']) is not int or value['exit_code'] != 0
            or value['proxy_env'] != 'unset' or value['fd3'] != 'closed'
            or any(not isinstance(value[key], str)
                   or re.fullmatch(r'net:\[[0-9]+\]', value[key]) is None
                   for key in ('outer_netns', 'inner_netns'))
            or value['outer_netns'] == value['inner_netns']):
        raise Refused('strict isolated execution observations unavailable')
    return value
