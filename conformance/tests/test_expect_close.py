"""Runtime-facing refusal cases require actual clean EOF, never silent peers."""
import asyncio
import json
from pathlib import Path
from types import SimpleNamespace
import sys
import tomllib
import unittest

CONF = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CONF))
from harness import cases_for, timeline, validate_case  # noqa: E402


class CloseValidation(unittest.TestCase):
    def test_close_is_explicit_true_only(self):
        validate_case({'name': 'closed', 'steps': [{'expect_close': True, 'within_ms': 20}]})
        for value in (False, None, 1, 'true', {}):
            with self.subTest(value=value), self.assertRaises(ValueError):
                validate_case({'name': 'closed', 'steps': [{'expect_close': value}]})

    def test_runtime_cases_have_opposite_role_and_explicit_profile(self):
        cases = cases_for(CONF, 'surface02-runtime')
        self.assertEqual(len(cases), 6)
        self.assertTrue(all(c['peer_role'] == 'surface' for c in cases))
        self.assertTrue(all(c['contract_profile'] == 'surface-0.2' for c in cases))
        self.assertFalse({c['name'] for c in cases} & {c['name'] for c in cases_for(CONF, 'surface02')})

    def test_runtime_registry_is_actual_daemon_with_exact_shared_catalog(self):
        data = tomllib.loads((CONF / 'impls-surface02-runtime.toml').read_text())
        self.assertEqual(len(data['implementations']), 1)
        impl = data['implementations'][0]
        self.assertEqual(impl['name'], 'runtime-cogletd-02')
        self.assertEqual(impl['suites'], ['surface02-runtime'])
        self.assertEqual(impl['peer_roles'], ['surface'])
        self.assertEqual(impl['contract_profile'], 'surface-0.2')
        self.assertEqual(impl['command'], ['python3', 'crates/coglet-runtime/tools/profile_fixture_bridge.py'])
        self.assertIs(impl['fixture_clock'], False)
        self.assertEqual(impl['ready_line'], 'COGLET_CONFORMANCE_READY')
        shared = tomllib.loads((CONF / 'impls-surface02.toml').read_text())['implementations'][0]['fixtures']['surface02-base']['env']
        self.assertEqual(set(impl['fixtures']), {'runtime02-' + x for x in ('base', 'read', 'rename', 'face')})
        for name, fixture in impl['fixtures'].items():
            env = fixture['env']
            for key, value in shared.items():
                self.assertEqual(env[key], value)
            setup = name.removeprefix('runtime02-')
            self.assertEqual(env['COGLET_FIXTURE_RUNTIME_SETUP'], f'conformance/surface02-runtime-fixtures/{setup}-setup.json')
            self.assertEqual(env['COGLET_FIXTURE_RUNTIME_BINDING'], 'conformance/surface02-runtime-fixtures/runtime-binding.json')

    def test_runtime_provisioning_reuses_exact_surface_payloads(self):
        def load(path):
            return json.loads((CONF / path).read_text())
        fixtures = 'surface02-runtime-fixtures/'
        read = load('surface02/app-read-state.json')['steps'][4]['send']['body']
        selected = load(fixtures + 'read-setup.json')['supervisor_reads'][0]
        self.assertEqual(selected['operation'], read['capability'])
        self.assertEqual(selected['args'], read['args'])
        self.assertEqual(selected['coglet'], read['coglet'])
        for setup, counterpart in [('rename', 'app-act-rename'), ('face', 'app-downgrade-builtin-action')]:
            with self.subTest(setup=setup):
                expected = load(f'surface02/{counterpart}.json')['steps'][4]['send']['body']['action']
                provisioned = load(fixtures + setup + '-setup.json')
                self.assertEqual(provisioned['initial_actions'], [expected])
                self.assertEqual(provisioned['initial_effect_ids']['actions'], [expected['id']])
        for path in (CONF / 'surface02-runtime-fixtures').glob('*-setup.json'):
            provisioned = json.loads(path.read_text())
            self.assertEqual(provisioned['contract_profile'], 'surface-0.2')
            self.assertEqual(provisioned['manifest']['version'], '0.1')
            self.assertNotIn('native_schedule', provisioned)  # no guest execution claim


class CloseTimeline(unittest.IsolatedAsyncioTestCase):
    async def run_reader(self, reader, process=None):
        peer = SimpleNamespace(impl={'transport': 'stdio'}, reader=reader, process=process)
        await timeline(peer, {'name': 'closed', 'steps': [{'expect_close': True, 'within_ms': 20}]})

    async def test_clean_eof_passes(self):
        reader = asyncio.StreamReader()
        reader.feed_eof()
        await self.run_reader(reader)

    async def test_child_crash_is_not_protocol_close(self):
        for status in (0, 7):
            with self.subTest(status=status):
                process = await asyncio.create_subprocess_exec(
                    sys.executable, str(CONF / 'tests/close_probe.py'), str(status),
                    stdout=asyncio.subprocess.PIPE)
                try:
                    # No dependency on interpreter startup speed in the wire deadline.
                    await asyncio.wait_for(process.wait(), 5)
                    if status:
                        with self.assertRaisesRegex(AssertionError, 'peer exited with status 7'):
                            await self.run_reader(process.stdout, process)
                    else:
                        await self.run_reader(process.stdout, process)
                finally:
                    if process.returncode is None:
                        process.kill()
                        await process.wait()

    async def test_silence_is_not_close(self):
        reader = asyncio.StreamReader()
        with self.assertRaisesRegex(AssertionError, 'TimeoutError'):
            await self.run_reader(reader)

    async def test_bytes_then_close_never_passes(self):
        for payload in (b'\x00', b'\x00\x00\x00\x01\xa0'):
            with self.subTest(payload=payload):
                reader = asyncio.StreamReader()
                reader.feed_data(payload)
                reader.feed_eof()
                with self.assertRaisesRegex(AssertionError, 'unexpected bytes'):
                    await self.run_reader(reader)

    async def test_reset_is_not_clean_eof(self):
        reader = asyncio.StreamReader()
        reader.set_exception(ConnectionResetError('reset'))
        with self.assertRaisesRegex(AssertionError, 'ConnectionResetError'):
            await self.run_reader(reader)
