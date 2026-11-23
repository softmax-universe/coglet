"""Exercise real child processes and sockets, plus hostile wire/config inputs."""
import asyncio
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
import xml.etree.ElementTree as ET

CONF = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CONF))
from harness import run_case, validate_case, validate_impl  # noqa: E402
from wire import decode, encode  # noqa: E402

RUN = CONF / 'run'
LOOPBACK = [sys.executable, str(CONF / 'loopback.py')]


class WireTests(unittest.TestCase):
    def test_all_conventions_goldens_and_wire_negatives(self):
        fixtures = CONF.parent / 'spec/fixtures/common'
        cases = json.loads((fixtures / 'cases.json').read_text())
        for case in cases:
            if case['valid']:
                with self.subTest(name=case['name']):
                    raw = bytes.fromhex(case['cbor_hex'])
                    self.assertEqual(encode(case['value']), raw)
                    self.assertEqual(encode(decode(raw)), raw)
        for case in json.loads((fixtures / 'wire-invalid.json').read_text()):
            with self.subTest(name=case['name']):
                with self.assertRaises((ValueError, UnicodeError)):
                    decode(bytes.fromhex(case['cbor_hex']))

    def test_type_and_integer_limits(self):
        for value in [1.0, b'x', 2**64, -2**63 - 1, {1: 'not-text'}]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                encode(value)
        self.assertNotEqual(encode(True), encode(1))
        self.assertEqual(decode(encode([2**64 - 1, -2**63])), [2**64 - 1, -2**63])

    def test_untrusted_large_counts_and_depth(self):
        for raw in [bytes.fromhex('9bffffffffffffffff'), bytes.fromhex('bbffffffffffffffff'), b'\x81' * 150 + b'\xf6']:
            with self.assertRaises(ValueError):
                decode(raw)


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.report = self.root / 'report.xml'

    def invoke(self, *args):
        result = subprocess.run([sys.executable, str(RUN), '--junit', str(self.report), *args],
                                cwd=self.root, capture_output=True, text=True, timeout=15)
        return result, ET.parse(self.report).getroot()

    def direct(self, case, command=LOOPBACK, **impl_options):
        impl = dict(name='test', transport='stdio', command=command, suites=['test'], **impl_options)
        validate_case(case)
        validate_impl(impl)
        return asyncio.run(run_case(impl, 'test', case))

    def case(self, steps, **options):
        return dict(name='test-case', steps=steps, **options)

    def test_self_test_and_junit_counts(self):
        result, report = self.invoke('--self-test')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(report.get('tests'), '4')
        self.assertEqual(report.get('failures'), '0')
        self.assertEqual(report.get('skipped'), '0')
        self.assertEqual(len(report.findall('testcase')), 4)

    def test_default_registry_runs_claimed_suites(self):
        import tomllib
        from harness import cases_for

        registry = tomllib.loads((CONF / 'impls.toml').read_text())
        expected = {
            (f"{impl['name']}.{suite}", case['name'])
            for impl in registry['implementations']
            for suite in impl['suites']
            for case in cases_for(CONF, suite)
        }
        result, report = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        actual = [(case.get('classname'), case.get('name'))
                  for case in report.findall('testcase')]
        self.assertEqual(set(actual), expected)
        self.assertEqual(len(actual), len(expected))
        self.assertEqual(report.get('tests'), str(len(expected)))
        self.assertEqual(report.get('failures'), '0')

    def test_empty_registry_no_claims(self):
        registry = self.root / 'impls.toml'
        registry.write_text('implementations = []\n')
        result, report = self.invoke('--registry', str(registry))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('No implementations registered', result.stdout)
        self.assertEqual(report.get('tests'), '0')

    def test_invalid_registry_and_missing_suite_fail_with_report(self):
        registry = self.root / 'impls.toml'
        for text in ['bad=1', '[[implementations]]\nname="bad"',
                     '[[implementations]]\nname="bad"\nsuites=["absent"]\ntransport="stdio"\ncommand=["missing"]']:
            with self.subTest(text=text):
                registry.write_text(text)
                result, report = self.invoke('--registry', str(registry))
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(report.get('failures'), '1')

    def test_direct_spawn_command(self):
        import shlex
        result, report = self.invoke('--impl', shlex.join(LOOPBACK), '--suite', 'self-test')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(report.get('tests'), '4')

    def test_missing_binary_and_crash_fail(self):
        case = self.case([{'send': {}}, {'expect': {}}])
        for cmd in [['./no-such-impl'], [sys.executable, '-c', 'raise SystemExit(7)']]:
            with self.subTest(cmd=cmd):
                self.assertIsNotNone(self.direct(case, cmd)[3])

    def test_mismatch_and_boolean_integer_distinction(self):
        for expected in [False, 1, {'wrong': True}]:
            self.assertIsNotNone(self.direct(self.case([{'send': True}, {'expect': expected}]))[3])

    def test_predicate_missing_path_and_wrong_hash_fail(self):
        for assertion in [{'match': {'missing.key': 1}}, {'journal_hash': '0' * 64}]:
            self.assertIsNotNone(self.direct(self.case([{'send': {'state_hash': '1' * 64}}, assertion]))[3])

    def test_journal_hash_is_real_deterministic_cbor_hash(self):
        expected = hashlib.sha256(encode({'counter': 3})).hexdigest()
        result = self.direct(self.case([{'send': {'loopback': 'hash', 'state': {'counter': 3}}},
                                        {'journal_hash': expected}]))
        self.assertIsNone(result[3], result[3])

    def test_deadline_and_whole_case_timeout(self):
        for case in [self.case([{'expect': {}, 'within_ms': 40}]),
                     self.case([{'send': {}, 'at_ms': 1000}], timeout_ms=40)]:
            started = time.monotonic()
            result = self.direct(case, [sys.executable, '-c', 'import time; time.sleep(30)'])
            self.assertIsNotNone(result[3])
            self.assertLess(time.monotonic() - started, 2)

    def test_scheduled_frame_and_silence(self):
        started = time.monotonic()
        result = self.direct(self.case([{'send': {}, 'at_ms': 50}, {'expect': {}}, {'silence_ms': 10}]))
        self.assertIsNone(result[3], result[3])
        self.assertGreaterEqual(time.monotonic() - started, .06)
        result = self.direct(self.case([{'send': {}}, {'silence_ms': 100}]))
        self.assertIsNotNone(result[3])

    def test_malformed_and_oversize_response_fail(self):
        for wire in [b'\x00\x00\x00\x02\x18\x17', b'\xff\xff\xff\xff', b'\x00\x00']:
            command = [sys.executable, '-c', f'import sys;sys.stdout.buffer.write({wire!r});sys.stdout.buffer.flush()']
            self.assertIsNotNone(self.direct(self.case([{'expect': 23}]), command)[3])

    def test_stderr_flood_does_not_deadlock(self):
        cmd = [sys.executable, '-c', 'import sys;sys.stderr.write("x"*200000);sys.stderr.flush();raise SystemExit(4)']
        result = self.direct(self.case([{'expect': {}}]), cmd)
        self.assertIsNotNone(result[3])
        self.assertLess(len(result[3] or ''), 9000)

    def test_stdout_flood_does_not_deadlock_cleanup(self):
        cmd = [sys.executable, '-c', 'import sys;sys.stdout.buffer.write(b"x"*2000000);sys.stdout.flush()']
        result = self.direct(self.case([{'expect': {}}]), cmd)
        self.assertIsNotNone(result[3])

    def test_unix_socket_same_wire_suite(self):
        path = str(self.root / 'peer.sock')
        process = subprocess.Popen(LOOPBACK + ['--socket', path], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 3
            while not Path(path).exists() and time.monotonic() < deadline:
                time.sleep(.01)
            self.assertTrue(Path(path).exists())
            result, report = self.invoke('--socket', path, '--suite', 'self-test')
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(report.get('tests'), '4')
        finally:
            process.terminate()
            process.communicate(timeout=3)

    def test_cbor_sequence_adapter(self):
        cmd = [sys.executable, '-c', 'import sys;sys.stdout.buffer.write(sys.stdin.buffer.read(1));sys.stdout.flush();sys.stdin.buffer.read()']
        result = self.direct(self.case([{'send': 3}, {'expect': 3}]), cmd, framing='cbor-sequence')
        self.assertIsNone(result[3], result[3])

    def test_fixture_and_role_unsupported_are_skipped_not_passed(self):
        for case in [self.case([{'send': {}}], fixtures=['enrolled']),
                     self.case([{'send': {}}], peer_role='hub')]:
            result = self.direct(case)
            self.assertIsNone(result[3])
            self.assertIsNotNone(result[4])

    def test_fixture_setup_env_teardown_and_failure_cleanup(self):
        marker = self.root / 'marker'
        hook = dict(setup=[sys.executable, '-c', f'from pathlib import Path;Path({str(marker)!r}).touch()'],
                    teardown=[sys.executable, '-c', f'from pathlib import Path;Path({str(marker)!r}).unlink()'],
                    env={'COGLET_FIXTURE_TEST': 'ready'})
        command = [sys.executable, '-c', 'import os,runpy,sys;assert os.environ["COGLET_FIXTURE_TEST"]=="ready";sys.path.insert(0,"conformance");runpy.run_path("conformance/loopback.py",run_name="__main__")']
        case = self.case([{'send': {}}, {'expect': {}}], fixtures=['enrolled'])
        result = self.direct(case, command, fixtures={'enrolled': hook})
        self.assertIsNone(result[3], result[3])
        self.assertFalse(marker.exists())
        hook['setup'] = [sys.executable, '-c', f'from pathlib import Path;Path({str(marker)!r}).touch();raise SystemExit(2)']
        result = self.direct(case, fixtures={'enrolled': hook})
        self.assertIsNotNone(result[3])
        self.assertFalse(marker.exists())

    def test_skip_is_encoded_in_junit(self):
        (self.root / 'test').mkdir()
        (self.root / 'test/case.json').write_text(json.dumps(self.case([{'send': {}}], fixtures=['unknown'])))
        registry = self.root / 'impls.toml'
        registry.write_text('[[implementations]]\nname="fixture"\nsuites=["test"]\ntransport="stdio"\ncommand=["missing"]\n')
        result, report = self.invoke('--registry', str(registry), '--cases', str(self.root))
        self.assertEqual(result.returncode, 0)
        self.assertEqual(report.get('skipped'), '1')
        self.assertIsNotNone(report.find('testcase/skipped'))
        self.assertIn('fixture unsupported', result.stdout)

    def test_registry_runs_every_implementation_suite_pair(self):
        for suite in ('alpha', 'beta'):
            (self.root / suite).mkdir()
            (self.root / suite / 'case.json').write_text(json.dumps(self.case([{'send': {}}, {'expect': {}}])))
        registry = self.root / 'impls.toml'
        registry.write_text(''.join(
            f'[[implementations]]\nname="{name}"\nsuites=["alpha","beta"]\ntransport="stdio"\ncommand={json.dumps(LOOPBACK)}\n'
            for name in ('one', 'two')))
        result, report = self.invoke('--registry', str(registry), '--cases', str(self.root))
        self.assertEqual(result.returncode, 0, result.stderr)
        pairs = {item.get('classname') for item in report.findall('testcase')}
        self.assertEqual(pairs, {'one.alpha', 'one.beta', 'two.alpha', 'two.beta'})
        self.assertEqual(report.get('tests'), '4')

    def test_stream_controls_require_iroh_and_valid_ids(self):
        result = self.direct(self.case([{'send': {}, 'stream': 0, 'finish': False}]))
        self.assertIn('require iroh', result[3] or '')
        for fields in [{'stream': -1}, {'stream': 2**63}, {'stream': True}, {'finish': False}, {'stream': 1, 'finish': 1}]:
            with self.subTest(fields=fields), self.assertRaises(ValueError):
                validate_case(self.case([dict(send={}, **fields)]))

    def test_dotted_fixture_labels_and_teardown_failure(self):
        result = self.direct(self.case([{'send': {}}, {'expect': {}}], fixtures=['hub.test-case']),
                             fixtures={'hub.test-case': {'teardown': [sys.executable, '-c', 'raise SystemExit(3)']}})
        self.assertIn('teardown hub.test-case', result[3] or '')
        self.assertIsNone(result[4])

    def test_xml_controls_are_sanitized(self):
        from harness import report
        report(self.report, [('test', 'bad\x01name', 0, 'error\x02message', None)])
        xml = ET.parse(self.report).getroot()
        self.assertEqual(xml.find('testcase').get('name'), 'badname')
        self.assertEqual(xml.find('testcase/failure').text, 'errormessage')

    def test_bad_case_and_duplicate_names_fail_preflight(self):
        for case in [self.case([{'typo': {}}]), self.case([{'send': {}, 'within_ms': -1}]),
                     self.case([{'send': {}, 'at_ms': 10}, {'send': {}, 'at_ms': 0}]),
                     self.case([{'send': 1.5}]), self.case([{'match': {}}])]:
            with self.subTest(case=case), self.assertRaises(ValueError):
                validate_case(case)
        (self.root / 'test').mkdir()
        case = self.case([{'send': {}}])
        (self.root / 'test/cases.json').write_text(json.dumps([case, case]))
        result, report = self.invoke('--impl', 'missing', '--suite', 'test', '--cases', str(self.root))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('duplicate case name', result.stderr)


if __name__ == '__main__':
    unittest.main()
