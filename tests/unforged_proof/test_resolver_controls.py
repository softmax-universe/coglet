"""Synthetic verifier regressions, not hosted qualification."""
import copy
import hashlib
import json
import unittest
from resolver_controls import verify_sequence, verify_delta
from hosted_receipts import Refused

A='a'*64
B='b'*64
P='c'*64
I='d'*64
T='e'*64
E='f'*64
IMAGE='native@sha256:'+'1'*64

def transcript():
    rows=[dict(endpoint_identity=E,same_client=True)]
    for label,resolver,hit in [('a-cold',A,False),('a-warm',A,True),('b-cold',B,False),('b-warm',B,True),('a-return',A,True)]:
        raw=f'tree {T}\nparent {P}\nparent {I}\nauthor synthetic\n\n{label}\nResolver-Commit: {resolver}\nMemo-Hit: {str(hit).lower()}\n'
        oid=hashlib.sha256(b'commit '+str(len(raw.encode())).encode()+b'\0'+raw.encode()).hexdigest()
        receipt=dict(runner='caos',worker_image=IMAGE,executed=not hit,cache='caos' if hit else 'none',commit=oid)
        rows.append(dict(label=label,resolver=resolver,memo_hit=hit,tree=T,commit=oid,raw_commit=raw,raw_receipt=json.dumps(receipt)))
    return rows

class ResolverControlTests(unittest.TestCase):
    def check(self,rows):
        return verify_sequence('\n'.join(json.dumps(r) for r in rows),program=P,inputs=I,resolver_a=A,resolver_b=B,endpoint=E,image=IMAGE,tree=T)

    def test_good_controls(self):
        self.assertEqual(len(self.check(transcript())),5)

    def test_missing_duplicate_and_reordered_observations(self):
        for mutate in (lambda r:r.pop(),lambda r:r.append(r[-1]),lambda r:r.reverse()):
            rows=transcript();mutate(rows)
            with self.assertRaises(Refused):self.check(rows)

    def test_every_disposition_is_required(self):
        for index in range(1,6):
            rows=transcript();rows[index]['memo_hit']=not rows[index]['memo_hit']
            with self.assertRaises(Refused):self.check(rows)

    def test_resolver_substitution_and_oracle_mismatch(self):
        for key,value in [('resolver',A),('tree','0'*64)]:
            rows=transcript();rows[3][key]=value
            with self.assertRaises(Refused):self.check(rows)

    def test_missing_ambiguous_or_changed_image_receipt(self):
        for raw in ('',transcript()[1]['raw_receipt']+'\n'+transcript()[1]['raw_receipt'],transcript()[1]['raw_receipt'].replace(IMAGE,'native@sha256:'+'0'*64)):
            rows=transcript();rows[1]['raw_receipt']=raw
            with self.assertRaises(Refused):self.check(rows)

    def test_duplicate_json_field(self):
        with self.assertRaises(Refused):
            verify_sequence('{"same_client":true,"same_client":false}',program=P,inputs=I,resolver_a=A,resolver_b=B,endpoint=E,image=IMAGE,tree=T)

    def test_delta_is_exactly_one_regular_metadata_file(self):
        old=b'100644 blob '+b'a'*64+b'\tstrategy\0'
        addition=b'100644 blob '+b'b'*64+b'\tstage2-synthetic-revision\0'
        metadata=b'coglet-stage2-resolver-control-v1\n'+b'1'*64+b'\n'
        verify_delta(old,old+addition,metadata)
        for new in (old,addition,old+addition.replace(b'100644',b'100755'),old.replace(b'a'*64,b'c'*64)+addition):
            with self.assertRaises(Refused):verify_delta(old,new,metadata)
        with self.assertRaises(Refused):verify_delta(old,old+addition,b'private arbitrary bytes')
