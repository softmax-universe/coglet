"""LOCAL parser tests; the real cargo graph check is explicit, never skipped."""
import unittest
from no_effects_closure import verify

GOOD='''coglet-unforged-proof v0.1.0 features=
coglet-runtime v0.1.0 features=native-engine
coglet-visualizer-native v0.1.0 features=
tokio v1.54.1 features=io-util,rt-multi-thread
'''

class ClosureTests(unittest.TestCase):
    def test_selected_graph_accepts_core_tokio(self):
        self.assertIn('tokio',verify(GOOD)['packages'])
    def test_each_live_transport_dependency_refuses(self):
        for name in ('coglet-cli','coglet-proto','coglet-llm','iroh','iroh-base','reqwest','hyper','socket2','mio','tokio-websockets'):
            with self.subTest(name=name),self.assertRaisesRegex(ValueError,'live transport'):
                verify(GOOD+name+' v1.0.0 features=\n')
    def test_runtime_device_and_missing_native_refuse(self):
        for features in ('device,native-engine','default,native-engine',''):
            with self.subTest(features=features),self.assertRaisesRegex(ValueError,'runtime features'):
                verify(GOOD.replace('features=native-engine','features='+features))
    def test_incomplete_or_unparseable_graph_refuses(self):
        for text in ('',GOOD.replace('coglet-runtime','other'),GOOD+'not a cargo row\n'):
            with self.subTest(text=text),self.assertRaises(ValueError):verify(text)

if __name__=='__main__':unittest.main()
