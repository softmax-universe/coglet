"""Opt-in testbed authority for private hosted proof exports.

Reuses Gate TrustKey/ArtifactStore, not production device authorization. Only the
operator holds this key, store and live revocation record. A candidate must not
construct this authority. Descriptors name an already validated complete reachable
closure (including private ancestry, code, input/reply and metadata objects).
Checking their identities against actual bytes/closure is the trusted caller's
job; this module does not turn unvalidated descriptors into content proof.
"""
import json
import re

from coglet_gate import Refused, canonical


def _scope(scope):
    if (not isinstance(scope, dict)
            or set(scope) != {'sender', 'recipient', 'coglet', 'objects', 'operations'}):
        raise Refused('closed private export scope required')
    for name in ('sender', 'recipient'):
        if not isinstance(scope[name], str) or re.fullmatch('[0-9a-f]{64}', scope[name]) is None:
            raise Refused('authenticated endpoint identity required')
    if (not isinstance(scope['coglet'], str)
            or re.fullmatch('coglet:[0-9a-f]{64}', scope['coglet']) is None
            or scope['operations'] != ['export', 'read']):
        raise Refused('explicit target export AND recipient read authority required')
    objects = scope['objects']
    if not isinstance(objects, list) or not 1 <= len(objects) <= 100000:
        raise Refused('bounded nonempty object closure required')
    seen = set()
    for obj in objects:
        if (not isinstance(obj, dict) or set(obj) != {'oid', 'kind', 'length'}
                or not isinstance(obj['oid'], str)
                or re.fullmatch('[0-9a-f]{64}', obj['oid']) is None
                or obj['kind'] not in ('blob', 'tree', 'commit', 'tag')
                or type(obj['length']) is not int or not 0 <= obj['length'] <= 128*1024*1024
                or obj['oid'] in seen):
            raise Refused('invalid or ambiguous closure descriptor')
        seen.add(obj['oid'])
    # Canonicalize metadata ONLY; no private bytes are read, serialized or written.
    return json.loads(canonical({**scope, 'objects': sorted(objects, key=lambda obj: obj['oid'])}))


class ExportAuthority:
    def __init__(self, store, key, revoked):
        self.store, self.key, self.revoked = store, key, revoked

    def issue(self, scope):
        return self.store.put(_scope(scope), self.key, 'hosted-private-export')

    def verify(self, ref, scope):
        expected = _scope(scope)
        if not isinstance(ref, str) or ref in self.revoked:
            raise Refused('private export grant missing or revoked')
        actual = self.store.read(ref, self.key, 'hosted-private-export')
        if actual != expected:
            raise Refused('private export closure/target/recipient mismatch')
        return actual


def export_private(authority, grant, scope, *, serialize, write, upload):
    """Keep denial ahead of payload access, then recheck at each crossing.

    Callbacks must use the verified retained closure, never candidate-selected
    replacements. A revocation after an authorized serialization cannot undo a
    read; rechecking before object writes and upload prevents further disclosure.
    This synchronous boundary is not a distributed revocation transaction.
    """
    scope = _scope(scope)
    authority.verify(grant, scope)
    payload = serialize()
    authority.verify(grant, scope)
    objects = write(payload)
    authority.verify(grant, scope)
    return upload(objects)
