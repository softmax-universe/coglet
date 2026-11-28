"""Local negative controls for exact worker sidecar bytes, not isolation proof."""
import hashlib
import json
import unittest

from diagnostic_receipts import verify_sidecar
from hosted_receipts import Refused


class DiagnosticSidecars(unittest.TestCase):
    def setUp(self):
        self.request = 'a'*40
        self.salt = 'b'*64
        self.raw = json.dumps(dict(request=self.request, salt=self.salt,
                                   netns='net:[123]', sandbox_env='unset',
                                   network_env='unset')).encode()+b'\n'
        self.oid = hashlib.sha1(b'blob '+str(len(self.raw)).encode()+b'\0'+self.raw).hexdigest()

    def test_exact_raw_blob_and_invocation_are_bound(self):
        got = verify_sidecar(self.raw, self.oid, request=self.request, salt=self.salt)
        self.assertEqual(got['network_env'], 'unset')
        self.assertNotIn('no_live_calls', got)

    def test_reformatted_or_wrong_oid_refuses(self):
        for raw, oid in [(self.raw.rstrip(), self.oid), (self.raw, 'c'*40)]:
            with self.assertRaises(Refused):
                verify_sidecar(raw, oid, request=self.request, salt=self.salt)

    def test_wrong_request_or_salt_refuses(self):
        for request, salt in [('c'*40, self.salt), (self.request, 'd'*64)]:
            with self.assertRaises(Refused):
                verify_sidecar(self.raw, self.oid, request=request, salt=salt)

    def test_duplicate_and_extra_observation_fields_refuse(self):
        for raw in [self.raw.replace(b'"netns":', b'"netns":"other", "netns":'),
                    self.raw.replace(b'"netns":', b'"invented_zero_count":0, "netns":')]:
            oid = hashlib.sha1(b'blob '+str(len(raw)).encode()+b'\0'+raw).hexdigest()
            with self.assertRaises(Refused):
                verify_sidecar(raw, oid, request=self.request, salt=self.salt)
