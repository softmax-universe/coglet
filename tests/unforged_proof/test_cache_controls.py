"""Synthetic transcript guards, not evidence of remote execution."""
import json
import unittest

from cache_controls import verify_cache_sequence
from hosted_receipts import Refused


class CacheControlRefusals(unittest.TestCase):
    def check(self, events):
        return verify_cache_sequence('\n'.join(json.dumps(e) for e in events),
            program='a'*64, inputs='b'*64, resolver='c'*64,
            endpoint='d'*64, image='native@sha256:'+'e'*64, tree='f'*64)

    def test_duplicate_envelope_fields_fail_closed(self):
        with self.assertRaisesRegex(Refused, 'duplicate'):
            verify_cache_sequence('{"same_client":false,"same_client":true}\n{}\n{}\n{}',
                program='a'*64, inputs='b'*64, resolver='c'*64,
                endpoint='d'*64, image='native@sha256:'+'e'*64, tree='f'*64)

    def test_missing_observation_fails_closed(self):
        with self.assertRaisesRegex(Refused, 'four'):
            self.check([])

    def test_changed_discovery_image_fails_closed(self):
        events = [dict(same_client=True, endpoint_identity='d'*64,
                       native_images=['native@sha256:'+'0'*64, None]), {}, {}, {}]
        with self.assertRaisesRegex(Refused, 'image'):
            self.check(events)

    def test_duplicate_labels_fail_closed(self):
        events = [dict(same_client=True, endpoint_identity='d'*64,
                       native_images=[None, None]),
                  dict(label='cold'), dict(label='cold'), dict(label='default-2')]
        with self.assertRaisesRegex(Refused, 'ordered'):
            self.check(events)
