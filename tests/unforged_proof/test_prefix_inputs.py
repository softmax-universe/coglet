"""Fast input-boundary guards; not hosted qualification."""
import copy
import gzip
import json
from pathlib import Path
import unittest
from prefix_inputs import step_payload

ROOT = Path(__file__).resolve().parents[2]

class PrefixInputs(unittest.TestCase):
    def setUp(self):
        self.trace = json.loads(gzip.decompress((ROOT/'tools/visualizer/native/fixtures/m2-gridbot-trace.json.gz').read_bytes()))

    def test_inputs_exclude_current_future_outputs_and_extra_blobs(self):
        original = copy.deepcopy(self.trace)
        self.trace['blobs']['f'*64] = 'private future blob'
        payload = step_payload(self.trace, 3)
        self.assertEqual(payload['prefix']['records'], original['records'][:3])
        self.assertEqual(set(payload['prefix']['blobs']), {original['code_hash'], original['manifest_hash']})
        self.assertNotIn('output_hash', payload['input'])
        self.assertNotIn('output_files', payload['input'])
        self.assertIsNone(payload['prefix']['trailing_boundary'])
        self.assertEqual(payload['input']['boundary'], original['records'][3]['boundary'])

    def test_first_prefix_is_initial_tree_not_final_tree(self):
        payload = step_payload(self.trace, 0)
        self.assertEqual(payload['prefix']['final_hash'], self.trace['initial_hash'])
        self.assertEqual(payload['prefix']['records'], [])
        self.assertEqual(set(payload), {'prefix','input'})
        self.assertNotIn('setup', payload)

    def test_invalid_step_index_fails_closed_without_mutation(self):
        original = copy.deepcopy(self.trace)
        for index in (-1, len(self.trace['records']), True, '0'):
            with self.subTest(index=index), self.assertRaises(ValueError):
                step_payload(self.trace, index)
        self.assertEqual(original, self.trace)
