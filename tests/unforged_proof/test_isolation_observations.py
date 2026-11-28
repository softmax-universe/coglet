"""Unit transcript fixtures, NOT worker execution or no-effects proof."""
import hashlib
import json
import unittest

from diagnostic_receipts import Refused, verify_isolation_sidecar


class IsolationObservations(unittest.TestCase):
    def check(self, **changed):
        value = dict(request='a'*40, salt='b'*64, outer_netns='net:[1]',
                     inner_netns='net:[2]', exit_code=0, proxy_env='unset', fd3='closed')
        value.update(changed)
        raw = json.dumps(value).encode()
        oid = hashlib.sha1(b'blob '+str(len(raw)).encode()+b'\0'+raw).hexdigest()
        return verify_isolation_sidecar(raw, oid, request='a'*40, salt='b'*64)

    def test_distinct_namespace_without_capability_routes(self):
        self.assertEqual(self.check()['exit_code'], 0)

    def test_strict_failure_never_falls_back(self):
        for exit_code in (1, 127, 125, False):
            with self.subTest(exit_code=exit_code), self.assertRaises(Refused):
                self.check(exit_code=exit_code)

    def test_same_or_missing_namespace_refuses(self):
        for namespace in ('net:[1]', '', 'unavailable', 'net:[not-an-inode]'):
            with self.subTest(namespace=namespace), self.assertRaises(Refused):
                self.check(inner_netns=namespace)

    def test_proxy_or_fd3_or_extra_claim_refuses(self):
        for change in (dict(proxy_env='set'), dict(fd3='open'), dict(qualified=True)):
            with self.subTest(change=change), self.assertRaises(Refused):
                self.check(**change)
