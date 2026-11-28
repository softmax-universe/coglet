"""Strict sidecar canary checks. Unit tests are not worker enforcement proof."""
import hashlib
import json
import unittest
from diagnostic_receipts import verify_loopback_sidecar
from hosted_receipts import Refused

class LoopbackCanaryChecks(unittest.TestCase):
    def value(self):
        return dict(request='a'*40, salt='b'*64, outer_netns='net:[1]', inner_netns='net:[2]',
                    exit_code=0, caos_server_url_env='unset', fd3='closed',
                    loopback_canary='1-1-0:refused,unreachable,loopback-unavailable:1-1-0')
    def verify(self, value):
        raw=json.dumps(value).encode()
        oid=hashlib.sha1(b'blob '+str(len(raw)).encode()+b'\0'+raw).hexdigest()
        return verify_loopback_sidecar(raw,oid,request='a'*40,salt='b'*64)
    def test_positive_baseline_and_denied_inner_observations(self):
        self.assertEqual(self.verify(self.value()),self.value())
    def test_missing_or_malformed_canary_fails_closed(self):
        for token in (None,'unavailable','1-1-0:refused,refused,loopback-up:1-1-0\n','1-1-0:error,refused,loopback-up:1-1-0'):
            value=self.value()
            if token is None: del value['loopback_canary']
            else: value['loopback_canary']=token
            with self.assertRaises(Refused): self.verify(value)
    def test_unisolated_connection_or_no_positive_baseline_refuses(self):
        for token in ('1-1-0:connected,refused,loopback-up:1-1-0','0-1-0:refused,refused,loopback-up:0-1-0','1-1-1:refused,refused,loopback-up:1-1-1'):
            value=self.value();value['loopback_canary']=token
            with self.assertRaises(Refused):self.verify(value)
    def test_extra_contact_after_replay_refuses(self):
        value=self.value();value['loopback_canary']='1-1-0:refused,refused,loopback-up:2-1-0'
        with self.assertRaises(Refused):self.verify(value)
