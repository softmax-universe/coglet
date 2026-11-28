"""Synthetic object-graph controls; these do not claim remote execution."""
import hashlib
import json
import unittest

from hosted_receipts import Refused, validate_invocation


def commit_object(tree, program, inputs, resolver):
    return (f'tree {tree}\nparent {program}\nparent {inputs}\n'
            'author proof <proof@example.invalid> 1 +0000\n'
            'committer proof <proof@example.invalid> 1 +0000\n\n'
            f'Merge-execution: program + input\n\nMemo-Hit: false\nResolver-Commit: {resolver}\n').encode()


def object_id(raw):
    return hashlib.sha256(b'commit ' + str(len(raw)).encode() + b'\0' + raw).hexdigest()


class HostedInvocationGuards(unittest.TestCase):
    tree, program, inputs, resolver = ('a'*64, 'b'*64, 'c'*64, 'd'*64)
    image = 'native@sha256:'+'e'*64

    def setUp(self):
        self.raw = commit_object(self.tree, self.program, self.inputs, self.resolver)
        self.oid = object_id(self.raw)
        self.note = json.dumps(dict(runner='caos', worker_image=self.image,
                                   executed=True, cache='none', commit=self.oid))

    def check(self, **changes):
        options = dict(raw_commit=self.raw, raw_receipt=self.note,
                       commit=self.oid, output_tree=self.tree,
                       program_commit=self.program, input_commit=self.inputs,
                       resolver=self.resolver, image=self.image,
                       memo_hit=False, fresh=True)
        options.update(changes)
        return validate_invocation(**options)

    def test_receipt_and_hashed_commit_graph_bind_expected_request(self):
        self.assertEqual(self.check()['commit'], self.oid)
        for change in ({'raw_commit':self.raw+b'changed'},
                       {'output_tree':'f'*64}, {'program_commit':'f'*64},
                       {'input_commit':'f'*64}, {'resolver':'f'*64}):
            with self.subTest(change=change), self.assertRaises(Refused):
                self.check(**change)

    def test_previous_identical_data_new_nonce_cannot_reuse_receipt(self):
        # A new nonce changes the retained input tree/commit despite identical
        # guest data. The old output and executed receipt are not fresh evidence.
        with self.assertRaises(Refused):
            self.check(input_commit='f'*64)

    def test_swapped_tree_cannot_hide_behind_valid_execution_receipt(self):
        wrong = commit_object('f'*64, self.program, self.inputs, self.resolver)
        oid = object_id(wrong)
        note = json.loads(self.note)
        note['commit'] = oid
        with self.assertRaises(Refused):
            self.check(raw_commit=wrong, raw_receipt=json.dumps(note), commit=oid)

    def test_duplicate_headers_and_wrong_parent_order_refuse(self):
        for raw in (self.raw.replace(b'\nauthor ', b'\ntree '+self.tree.encode()+b'\nauthor '),
                    commit_object(self.tree, self.inputs, self.program, self.resolver)):
            oid = object_id(raw)
            note = json.loads(self.note)
            note['commit'] = oid
            with self.subTest(raw=raw), self.assertRaises(Refused):
                self.check(raw_commit=raw, raw_receipt=json.dumps(note), commit=oid)


if __name__ == '__main__':
    unittest.main()
