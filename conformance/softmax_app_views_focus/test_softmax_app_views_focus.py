"""Static guards for conformance/softmax_app_views_focus (stdlib only).

Run: python3 -m unittest discover -s conformance/softmax_app_views_focus -p 'test_*.py' -v
The wire behaviour itself is exercised by the harness (see README.md), not here.
When cbor2 is importable, test_harness_run also executes the suite end to end.
"""
import hashlib, json, subprocess, sys, tomllib, unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
CATALOG = ROOT / 'apps/softmax-mac/Surface/catalog.v1.json'
VIEWS_FOCUS_ACTIONS = {'navigate', 'open_window', 'close_window', 'set_focus'}


def cases():
    out = []
    for p in sorted(HERE.glob('*.json')):
        v = json.loads(p.read_text())
        out += v if isinstance(v, list) else [v]
    return out


class Static(unittest.TestCase):
    def test_pin_is_the_real_catalog_digest(self):
        entry = tomllib.loads((HERE / 'impls.toml').read_text())['implementations'][0]
        pin = entry['fixtures']['softmax-app-base']['env']['COGLET_FIXTURE_CATALOG_SHA256']
        self.assertEqual(pin, hashlib.sha256(CATALOG.read_bytes()).hexdigest())
        self.assertEqual(pin, (CATALOG.parent / 'catalog.v1.sha256').read_text().strip())
        self.assertEqual((entry['suites'], entry['contract_profile']), (['softmax_app_views_focus'], 'surface-0.2'))

    def test_fixture_grants_exactly_the_four_actions(self):
        cfg = json.loads((HERE / 'adapter-config.fixture').read_text())
        ops = cfg['grant']['entries'][0]['operations']
        self.assertEqual(set(ops), {'softmax.app.' + a for a in VIEWS_FOCUS_ACTIONS})
        self.assertEqual(cfg['grant']['entries'][0]['resources'], ['softmax.app_control'])

    def test_cases_are_wire_cases_on_surface02_only(self):
        cs = cases()
        self.assertGreaterEqual(len(cs), 9)
        names = [c['name'] for c in cs]
        self.assertEqual(len(names), len(set(names)))
        catalog = json.loads(CATALOG.read_text())['operations']
        used = set()
        for c in cs:
            self.assertEqual((c['contract_profile'], c['peer_role'], c['fixtures']), ('surface-0.2', 'runtime', ['softmax-app-base']))
            kinds = {k for s in c['steps'] for k in s}
            self.assertTrue(kinds & {'send', 'match'}, c['name'])
            used |= {s['send']['body']['action']['capability'] for s in c['steps'] if s.get('send', {}).get('type') == 'act'}
        # Every positive case targets a real catalog operation of the four-action set.
        positive = {u for u in used if u in catalog}
        for a in VIEWS_FOCUS_ACTIONS:
            self.assertIn('softmax.app.' + a, positive)

    def test_frozen_cases_untouched_by_this_suite(self):
        # Suite directory must hold no copies of frozen 0.1 surface/speech case names.
        frozen = set()
        for d in ('surface', 'speech'):
            for p in (ROOT / 'conformance' / d).glob('**/*.json'):
                v = json.loads(p.read_text())
                frozen |= {c['name'] for c in (v if isinstance(v, list) else [v])}
        self.assertFalse(frozen & {c['name'] for c in cases()})

    @unittest.skipUnless(__import__('importlib').util.find_spec('cbor2'), 'cbor2 not importable')
    def test_harness_run(self):
        r = subprocess.run([sys.executable, str(ROOT / 'conformance/run'),
                            '--registry', str(HERE / 'impls.toml'), '--contract-profile', 'surface-0.2',
                            '--suite', 'softmax_app_views_focus', '--junit', str(Path(__import__('tempfile').gettempdir()) / 'softmax-app-views-focus.xml')],
                           cwd=ROOT, capture_output=True, text=True, timeout=240)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(r.stdout.count('PASS '), len(cases()), r.stdout)


if __name__ == '__main__':
    unittest.main()
