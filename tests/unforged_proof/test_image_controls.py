"""LOCAL image-receipt verifier controls; not hosted execution evidence."""
import copy
import unittest
from image_controls import verify_headers, verify_sequence, Refused

A='localhost:5000/caos@sha256:'+'a'*64
B='localhost:5000/caos@sha256:'+'b'*64

class ImageControlsTests(unittest.TestCase):
    def rows(self):
        return [dict(label=label,request=req*40,args='c'*40,bundle='d'*40,
                     resolver='e'*64,driver='f'*64,salt='1'*64,
                     image=image,headers=[['X-Caos-Cache',cache],['X-Caos-Image',image]],
                     tree='2'*64) for label,req,image,cache in [
                         ('a-cold','a',A,'miss'),('b-first','b',B,'miss'),
                         ('b-warm','b',B,'hit'),('a-return','a',A,'hit')]]
    def verify(self,rows):
        return verify_sequence(rows,image_a=A,image_b=B,resolver='e'*64,
            driver='f'*64,salt='1'*64,tree='2'*64)
    def test_real_header_pairs_required(self):
        self.assertEqual(verify_headers(self.rows()[0]['headers'],A,False),'none')
        for headers in [[],[['X-Caos-Cache','miss']],self.rows()[0]['headers']+[['x-caos-cache','miss']],
                        [['X-Caos-Cache','unknown'],['X-Caos-Image',A]],
                        [['X-Caos-Cache','miss'],['X-Caos-Image',B]]]:
            with self.subTest(headers=headers),self.assertRaises(Refused): verify_headers(headers,A,False)
    def test_exact_four_same_arg_observations(self):
        self.assertEqual(self.verify(self.rows()),dict(executions=2,remote_hits=2))
    def test_missing_duplicate_or_reordered_receipt_refuses(self):
        rows=self.rows()
        for bad in [rows[:-1],rows+[rows[0]],[rows[1],rows[0],*rows[2:]]]:
            with self.assertRaises(Refused): self.verify(bad)
    def test_changed_program_input_resolver_or_salt_refuses(self):
        for key in ['args','bundle','resolver','driver','salt','tree']:
            bad=self.rows();bad[1][key]='0'*len(bad[1][key])
            with self.subTest(key=key),self.assertRaises(Refused):self.verify(bad)
    def test_b_first_hit_and_warm_miss_refuse(self):
        for i in range(4):
            bad=self.rows();bad[i]['headers'][0][1]='hit' if i<2 else 'miss'
            with self.subTest(i=i),self.assertRaises(Refused):self.verify(bad)
    def test_request_aliasing_and_wrong_return_refuse(self):
        for i,new in [(1,'a'*40),(2,'c'*40),(3,'b'*40)]:
            bad=self.rows();bad[i]['request']=new
            with self.assertRaises(Refused):self.verify(bad)
    def test_wrong_image_or_malformed_row_refuses(self):
        for replacement in [None,{},dict(self.rows()[1],image=A)]:
            bad=self.rows();bad[1]=replacement
            with self.assertRaises(Refused):self.verify(bad)
