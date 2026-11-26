"""Fast guard tests; run without Rust, Git 2.45, or unforged."""
import copy
import unittest
from run import comparison, prefix, receipt, require


class ProofGuards(unittest.TestCase):
    def test_prefix_excludes_current_and_future_outputs(self):
        trace = {'initial_hash':'initial', 'final_hash':'future', 'records':[
            {'output_hash':'prior'}, {'output_hash':'current-secret'}],
            'code_hash':'code','manifest_hash':'manifest',
            'blobs':{'code':'wasm','manifest':'manifest','future':'secret'},
            'trailing_boundary':{'future':'secret'}}
        original = copy.deepcopy(trace)
        pre = prefix(trace,1)
        self.assertEqual(pre['final_hash'],'prior')
        self.assertEqual(len(pre['records']),1)
        self.assertNotIn('current-secret',str(pre))
        self.assertEqual(set(pre['blobs']),{'code','manifest'})
        self.assertIsNone(pre['trailing_boundary'])
        self.assertEqual(trace,original)
        self.assertEqual(prefix(trace,0)['final_hash'],'initial')

    def test_equality_and_execution_are_both_mandatory(self):
        a, b = 'a'*64, 'b'*64
        def text(tree, hit):
            return f'CONFLUENCE commit tree={tree} memo_hit={str(hit).lower()} resolver=pin'
        self.assertTrue(comparison(0,a,text(a,False),text(a,True))['pass'])
        for cold, warm in [(text(b,False),text(a,True)),(text(a,False),text(b,True)),
                           (text(a,True),text(a,True)),(text(a,False),text(a,False))]:
            with self.subTest(cold=cold,warm=warm):
                self.assertFalse(comparison(0,a,cold,warm)['pass'])

    def test_missing_receipts_fail_closed(self):
        for text in ['', 'PASS', 'tree=abc memo_hit=true','tree='+'a'*64]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                receipt(text)

    def test_checks_survive_python_optimization(self):
        with self.assertRaises(RuntimeError):
            require(False,'mismatch')


if __name__ == '__main__':
    unittest.main()
