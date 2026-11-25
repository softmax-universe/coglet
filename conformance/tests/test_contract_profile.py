"""ADR-0030 section 1a: harness contract_profile selection (stdlib only, real child processes)."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest

CONF = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CONF))
from harness import (PROFILE_ENV, cases_for, check_case_profile, registry, select_profiles,  # noqa: E402
                     validate_case, validate_impl)

RUN = CONF / 'run'
PROBE = CONF / 'tests/profile_probe.py'
BAD = ['', None, 'surface-0.3', 'SURFACE-0.2', ' surface-0.2', 'surface-0.2 ', '0.2', 0.2, ['surface-0.2'], 'surface-0.2\n']


def impl(**extra):
    return dict(name='probe', suites=['probe'], transport='stdio', command=[sys.executable, str(PROBE)], **extra)


def case(name='report', want='surface-0.1', **extra):
    return dict(name=name, steps=[{'expect': {'profile': want}}], **extra)


class Workspace:
    """A temp dir with a probe suite, registry and launch marker."""

    def __init__(self, test, cases, **impl_fields):
        self.dir = tempfile.TemporaryDirectory()
        test.addCleanup(self.dir.cleanup)
        root = Path(self.dir.name)
        (root / 'cases/probe').mkdir(parents=True)
        (root / 'cases/probe/c.json').write_text(json.dumps(cases))
        self.marker, self.junit = root / 'marker', root / 'out.xml'
        self.registry = root / 'impls.toml'
        fields = ''.join(f'{k} = {json.dumps(v)}\n' for k, v in impl_fields.items())
        self.registry.write_text('[[implementations]]\nname = "probe"\nsuites = ["probe"]\ntransport = "stdio"\n'
                                 f'command = {json.dumps([sys.executable, str(PROBE)])}\n{fields}')
        self.cases = root / 'cases'

    def run(self, *extra, env=None):
        full = dict(os.environ, COGLET_PROBE_MARKER=str(self.marker))
        full.update(env or {})
        return subprocess.run([sys.executable, str(RUN), '--registry', str(self.registry), '--cases', str(self.cases),
                               '--junit', str(self.junit), *extra], capture_output=True, text=True, env=full, timeout=60)

    @property
    def launched(self):
        return self.marker.exists()


class Validation(unittest.TestCase):
    def test_impl_profile_default_and_exact_spellings(self):
        validate_impl(impl())
        validate_impl(impl(contract_profile='surface-0.1'))
        validate_impl(impl(contract_profile='surface-0.2'))
        for bad in BAD:
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                validate_impl(impl(contract_profile=bad))

    def test_case_profile_exact_spellings(self):
        validate_case(case(contract_profile='surface-0.2'))
        for bad in BAD:
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                validate_case(case(contract_profile=bad))

    def test_provisioning_is_launch_time_so_attached_transports_cannot_opt_in(self):
        validate_impl(dict(name='u', suites=['s'], transport='unix', socket='/tmp/x', contract_profile='surface-0.1'))
        with self.assertRaises(ValueError):
            validate_impl(dict(name='u', suites=['s'], transport='unix', socket='/tmp/x', contract_profile='surface-0.2'))
        with self.assertRaises(ValueError):
            validate_impl(dict(name='i', suites=['s'], transport='iroh', endpoint='0' * 64, alpn='x',
                               contract_profile='surface-0.2'))

    def test_fixture_hooks_cannot_set_the_profile_variable(self):
        with self.assertRaises(ValueError):
            validate_impl(impl(fixtures={'label': {'env': {PROFILE_ENV: 'surface-0.2'}}}))

    def test_cli_selector_must_agree_with_every_selected_entry(self):
        a, b = impl(), dict(impl(), name='other', contract_profile='surface-0.2')
        self.assertEqual(select_profiles([a], None), [a])
        select_profiles([a], 'surface-0.1')
        select_profiles([b], 'surface-0.2')
        for entries, override in (([a], 'surface-0.2'), ([b], 'surface-0.1'), ([a, b], 'surface-0.1'), ([a, b], 'surface-0.2')):
            with self.subTest(override=override, n=len(entries)), self.assertRaisesRegex(ValueError, 'conflicts'):
                select_profiles(entries, override)
        with self.assertRaises(ValueError):
            select_profiles([a], 'surface-0.3')

    def test_case_profile_must_match_implementation(self):
        check_case_profile(impl(), 'probe', case())
        check_case_profile(impl(contract_profile='surface-0.2'), 'probe', case(contract_profile='surface-0.2'))
        for i, c in ((impl(), case(contract_profile='surface-0.2')), (impl(contract_profile='surface-0.2'), case())):
            with self.assertRaisesRegex(ValueError, 'requires'):
                check_case_profile(i, 'probe', c)


class ShippedFiles(unittest.TestCase):
    def test_frozen_suites_are_untouched_surface_01(self):
        for suite in ('surface', 'speech'):
            for c in cases_for(CONF, suite):
                self.assertNotIn('contract_profile', c, f'{suite}/{c["name"]} must stay implicit 0.1')
        for entry in registry(CONF / 'impls.toml'):
            self.assertNotEqual(entry.get('contract_profile', 'surface-0.1'), 'surface-0.2', entry['name'])

    def test_surface02_cases_are_separately_selected(self):
        cases = cases_for(CONF, 'surface02')
        self.assertTrue(cases)
        self.assertTrue(all(c['contract_profile'] == 'surface-0.2' for c in cases))
        frozen = {c['name'] for c in cases_for(CONF, 'surface')}
        self.assertFalse(frozen & {c['name'] for c in cases})

    def test_surface02_registry_claims_only_real_adapters(self):
        entries = registry(CONF / 'impls-surface02.toml')
        self.assertEqual({e['name'] for e in entries}, {'surface-python-reference-02', 'surface-go-reference-02'})
        for e in entries:
            self.assertEqual((e['contract_profile'], e['suites']), ('surface-0.2', ['surface02']))
        # The catalog fixture digest is the pin supplied to adapters, never derived from the artifact.
        import hashlib
        raw = (CONF / 'surface02-fixtures/app-catalog.json').read_bytes()
        pins = {e['fixtures']['surface02-base']['env']['COGLET_FIXTURE_CATALOG_SHA256'] for e in entries}
        self.assertEqual(pins, {hashlib.sha256(raw).hexdigest()})
        self.assertNotIn('contract_profile', json.loads(raw))
        tomllib.loads((CONF / 'impls-surface02.toml').read_text())

    def test_swift_surface02_uses_same_cases_and_pin_without_claiming_runtime(self):
        entries = registry(CONF / 'impls-surface02-swift.toml')
        self.assertEqual(len(entries), 1)
        entry = entries[0]
        self.assertEqual(entry['name'], 'surface-swift-reference-02')
        self.assertEqual(entry['suites'], ['surface02'])
        self.assertEqual(entry['peer_roles'], ['runtime'])  # harness role; implementation is a surface
        self.assertEqual(entry['contract_profile'], 'surface-0.2')
        self.assertFalse(entry['fixture_clock'])  # production real clock, unchanged wire deadlines
        self.assertEqual(entry['ready_line'], 'COGLET_CONFORMANCE_READY')
        self.assertEqual(entry['fixtures'], registry(CONF / 'impls-surface02.toml')[0]['fixtures'])


class EndToEnd(unittest.TestCase):
    def test_default_launch_provisions_surface_01_even_if_the_ambient_env_says_otherwise(self):
        ws = Workspace(self, [case(want='surface-0.1')])
        for ambient in (None, 'surface-0.2', ''):
            with self.subTest(ambient=ambient):
                env = {} if ambient is None else {PROFILE_ENV: ambient}
                done = ws.run(env=env)
                self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
                self.assertIn('PASS probe.probe/report', done.stdout)

    def test_registry_entry_selects_profile_and_cli_agreement_is_accepted(self):
        ws = Workspace(self, [case(want='surface-0.2', contract_profile='surface-0.2')], contract_profile='surface-0.2')
        for extra in ((), ('--contract-profile', 'surface-0.2')):
            with self.subTest(extra=extra):
                done = ws.run(*extra)
                self.assertEqual(done.returncode, 0, done.stdout + done.stderr)

    def test_conflicts_fail_before_any_launch(self):
        scenarios = [
            ('cli 0.2 vs default entry', [case()], {}, ('--contract-profile', 'surface-0.2'), 'conflicts'),
            ('cli 0.1 vs 0.2 entry', [case(contract_profile='surface-0.2')], {'contract_profile': 'surface-0.2'},
             ('--contract-profile', 'surface-0.1'), 'conflicts'),
            ('case 0.2 on default entry', [case(contract_profile='surface-0.2')], {}, (), 'requires surface-0.2'),
            ('case 0.1 on 0.2 entry', [case()], {'contract_profile': 'surface-0.2'}, (), 'requires surface-0.1'),
            ('bad registry spelling', [case()], {'contract_profile': 'surface-0.3'}, (), 'contract_profile'),
            ('empty registry spelling', [case()], {'contract_profile': ''}, (), 'contract_profile'),
        ]
        for label, cases, fields, extra, message in scenarios:
            with self.subTest(label):
                ws = Workspace(self, cases, **fields)
                done = ws.run(*extra)
                self.assertEqual(done.returncode, 1, done.stdout + done.stderr)
                self.assertIn(message, done.stderr)
                self.assertFalse(ws.launched, 'adapter was launched despite a profile conflict')

    def test_cli_rejects_unknown_profile_before_anything_else(self):
        ws = Workspace(self, [case()])
        done = ws.run('--contract-profile', 'surface-0.3')
        self.assertEqual(done.returncode, 2, done.stderr)
        self.assertFalse(ws.launched)

    def test_direct_mode_and_self_test_use_the_same_selector(self):
        ws = Workspace(self, [case(want='surface-0.2', contract_profile='surface-0.2')])
        command = f'{sys.executable} {PROBE}'
        base = [sys.executable, str(RUN), '--impl', command, '--suite', 'probe', '--cases', str(ws.cases),
                '--junit', str(ws.junit)]
        env = dict(os.environ, COGLET_PROBE_MARKER=str(ws.marker))
        done = subprocess.run([*base, '--contract-profile', 'surface-0.2'], capture_output=True, text=True, env=env, timeout=60)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        ws.marker.unlink()
        done = subprocess.run(base, capture_output=True, text=True, env=env, timeout=60)  # omitted means 0.1
        self.assertEqual(done.returncode, 1)
        self.assertFalse(ws.launched)
        done = subprocess.run([sys.executable, str(RUN), '--self-test', '--contract-profile', 'surface-0.2',
                               '--junit', str(ws.junit)], capture_output=True, text=True, timeout=60)
        self.assertEqual(done.returncode, 1)
        self.assertIn('conflicts', done.stderr)

    def test_profile_travels_by_env_not_by_wire(self):
        # The probe only ever sees the env value; the harness sends no frame at all in this case.
        ws = Workspace(self, [case(want='surface-0.2', contract_profile='surface-0.2')], contract_profile='surface-0.2')
        done = ws.run()
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)


if __name__ == '__main__':
    unittest.main()
