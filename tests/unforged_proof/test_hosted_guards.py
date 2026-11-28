"""Fast synthetic boundary tests, NOT hosted execution qualification."""
import json
import unittest

from hosted_receipts import Refused, validate_receipt


class HostedReceiptGuards(unittest.TestCase):
    commit = 'a' * 64
    image = 'native@sha256:' + 'b' * 64

    def note(self, **changes):
        value = dict(runner='caos', worker_image=self.image,
                     executed=True, cache='none', commit=self.commit)
        value.update(changes)
        return json.dumps(value)

    def check(self, raw, **options):
        args = dict(commit=self.commit, image=self.image, memo_hit=False, fresh=True)
        args.update(options)
        return validate_receipt(raw, **args)

    def test_cold_requires_remote_execution_and_exact_identity(self):
        self.assertEqual(self.check(self.note())['commit'], self.commit)
        for change in (dict(runner='unforged'), dict(worker_image=None),
                       dict(worker_image='native@sha256:' + 'c'*64),
                       dict(commit='d'*64), dict(executed=False),
                       dict(cache='local'), dict(cache='caos'),
                       dict(executed=1), dict(executed='true')):
            with self.subTest(change=change), self.assertRaises(Refused):
                self.check(self.note(**change))
        with self.assertRaises(Refused):
            self.check(self.note(), memo_hit=True)

    def test_missing_duplicate_ambiguous_and_unknown_fields_refuse(self):
        for raw in ('', '{}', self.note()+'\n'+self.note(),
                    self.note()[:-1]+',"executed":false}',
                    self.note()[:-1]+',"extra":"ignored"}',
                    '{broken}', '[]', 'null'):
            with self.subTest(raw=raw), self.assertRaises(Refused):
                self.check(raw)

    def test_warm_controls_distinguish_local_and_remote_hits(self):
        for cache in ('local', 'caos'):
            result = self.check(self.note(executed=False, cache=cache),
                                memo_hit=True, fresh=False)
            self.assertEqual(result['cache'], cache)
            with self.assertRaises(Refused):
                self.check(self.note(executed=False, cache=cache))
        for executed, cache, hit in ((True, 'caos', True),
                                     (False, 'none', True),
                                     (False, 'local', False),
                                     (True, 'none', True)):
            with self.subTest(executed=executed, cache=cache, hit=hit), self.assertRaises(Refused):
                self.check(self.note(executed=executed, cache=cache),
                           memo_hit=hit, fresh=False)

    def test_configured_name_or_sentinel_is_not_image_identity(self):
        for image in ('unforged', 'worker:latest', 'a'*40,
                      'unforged@sha256:756e666f726765643a63616f732d72756e6e65723a76312e3100000000000000',
                      'native@sha256:'+'B'*64, 'native@sha256:'+'b'*63):
            with self.subTest(image=image), self.assertRaises(Refused):
                self.check(self.note(worker_image=image), image=image)

    def test_control_flags_require_booleans(self):
        for flags in ({'fresh': 'true'}, {'memo_hit': 0}, {'fresh': 1}):
            with self.subTest(flags=flags), self.assertRaises(Refused):
                self.check(self.note(), **flags)


if __name__ == '__main__':
    unittest.main()
