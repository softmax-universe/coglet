"""Verify actual diagnostic sidecar bytes and invocation, not effective isolation.

The caller must obtain the OID from the authenticated completed status of a fresh
request whose execution receipt/image it has independently verified. This helper
alone does not authenticate status, prove an execution, or qualify a gate.
"""
import hashlib
import json
import re

from hosted_receipts import Refused


def verify_sidecar(raw, oid, *, request, salt):
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
    if (not isinstance(value, dict)
            or set(value) != {'request', 'salt', 'netns', 'sandbox_env', 'network_env'}
            or any(not isinstance(item, str) for item in value.values())
            or value['request'] != request or value['salt'] != salt):
        raise Refused('closed diagnostic observations or invocation mismatch')
    return value
