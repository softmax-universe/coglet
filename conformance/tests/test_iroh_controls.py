"""Private iroh controls and durable post-wire fixture verification."""
import asyncio
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from harness import Peer, run_case, validate_impl


class IrohControlTests(unittest.TestCase):
    def implementation(self, **extra):
        return dict(name='router-test', suites=['hub'], transport='iroh',
                    endpoint='03' * 32, alpn='coglet/hub/0', **extra)

    def test_accept_and_incoming_stream_validate(self):
        validate_impl(self.implementation(iroh_mode='accept', address='127.0.0.1:3000',
                                          incoming_stream=100))
        validate_impl(self.implementation(incoming_stream=2**64 - 1))
        for extra in [dict(iroh_mode='listen'), dict(incoming_stream=True),
                      dict(incoming_stream=-1), dict(incoming_stream=2**64),
                      dict(iroh_mode='accept')]:
            with self.subTest(extra=extra), self.assertRaises(ValueError):
                validate_impl(self.implementation(**extra))
        for key, value in [('iroh_mode', 'accept'), ('incoming_stream', 100)]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                validate_impl(dict(name='bad', suites=['hub'], transport='stdio',
                                   command=['unused'], **{key: value}))

    def test_driver_argv_is_explicit_and_opaque(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            args = root / 'args.json'
            driver = root / 'driver'
            driver.write_text('#!' + sys.executable + '\nimport json, sys\n'
                              f'open({str(args)!r}, "w").write(json.dumps(sys.argv[1:]))\n'
                              'sys.stdin.buffer.read()\n')
            driver.chmod(0o700)
            impl = self.implementation(driver=str(driver), iroh_mode='accept',
                                       address='127.0.0.1:3000', incoming_stream=100)
            async def check():
                peer = Peer(impl)
                await peer.start()
                try:
                    assert peer.writer is not None and peer.process is not None
                    peer.writer.close()
                    await peer.process.wait()
                    self.assertEqual(json.loads(args.read_text()), [
                        '--endpoint', '03' * 32, '--alpn', 'coglet/hub/0',
                        '--address', '127.0.0.1:3000', '--mode', 'accept',
                        '--incoming-stream', '100'])
                finally:
                    await peer.close()
            asyncio.run(check())


    def test_driver_final_drain_rejects_failure_and_trailing_bytes(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            driver = root / 'driver'
            for status, trailing in [(0, False), (7, False), (0, True)]:
                with self.subTest(status=status, trailing=trailing):
                    driver.write_text('#!' + sys.executable + '\nimport sys\n'
                                      'i, o = sys.stdin.buffer, sys.stdout.buffer\n'
                                      'size = int.from_bytes(i.read(4), "big")\n'
                                      'raw = i.read(size)[9:]\n'
                                      'o.write(len(raw).to_bytes(4, "big") + raw); o.flush()\n'
                                      'i.read()\n'
                                      f'o.write({b"x" if trailing else b""!r}); o.flush()\n'
                                      f'raise SystemExit({status})\n')
                    driver.chmod(0o700)
                    impl = self.implementation(driver=str(driver))
                    case = dict(name='drain', steps=[{'send': {}}, {'expect': {}}])
                    result = asyncio.run(run_case(impl, 'hub', case))
                    if status:
                        self.assertIn('wire peer exited 7', result[3] or '')
                    elif trailing:
                        self.assertIn('trailing driver output', result[3] or '')
                    else:
                        self.assertIsNone(result[3])


class FixtureVerificationTests(unittest.TestCase):
    def test_stdio_fixture_requires_clean_exit_when_opted_in(self):
        with tempfile.TemporaryDirectory() as root:
            script = Path(root) / 'peer.py'
            script.write_text('import sys\n'
                              'i, o = sys.stdin.buffer, sys.stdout.buffer\n'
                              'h = i.read(4); raw = i.read(int.from_bytes(h, "big"))\n'
                              'o.write(h + raw); o.flush()\n'
                              'i.read()\nraise SystemExit(7)\n')
            impl = dict(name='late-failure', transport='stdio', suites=['test'],
                        command=[sys.executable, str(script)], finish_on_eof=True)
            validate_impl(impl)
            case = dict(name='late-failure', steps=[{'send': {}}, {'expect': {}}])
            result = asyncio.run(run_case(impl, 'test', case))
            self.assertIn('wire peer exited 7', result[3] or '')
            for value in (0, 'true', None):
                with self.subTest(value=value), self.assertRaises(ValueError):
                    validate_impl(dict(impl, finish_on_eof=value))

    def test_post_wire_oracle_is_required_and_precedes_teardown(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            order = root / 'order'
            hook = root / 'hook.py'
            hook.write_text('import pathlib, sys\n'
                            f'p = pathlib.Path({str(order)!r})\n'
                            'with p.open("a") as f: f.write(sys.argv[1] + "\\n")\n'
                            'raise SystemExit(int(sys.argv[2]))\n')
            for status in (0, 7):
                with self.subTest(status=status):
                    order.unlink(missing_ok=True)
                    impl = dict(name='oracle-test', transport='stdio', suites=['test'],
                                command=[sys.executable, str(Path(__file__).resolve().parents[1] / 'loopback.py')],
                                fixtures={'durable-oracle': {
                                    'verify': [sys.executable, str(hook), 'verify', str(status)],
                                    'teardown': [sys.executable, str(hook), 'teardown', '0']}})
                    validate_impl(impl)
                    case = dict(name='oracle', fixtures=['durable-oracle'],
                                steps=[{'send': {'ok': True}}, {'expect': {'ok': True}}])
                    result = asyncio.run(run_case(impl, 'test', case))
                    self.assertEqual(order.read_text(), 'verify\nteardown\n')
                    self.assertIsNone(result[4])
                    if status:
                        assert result[3] is not None
                        self.assertIn('fixture hook exited 7', result[3])
                    else:
                        self.assertIsNone(result[3])

    def test_failed_wire_does_not_invoke_success_oracle_but_cleans_up(self):
        with tempfile.TemporaryDirectory() as root:
            marker = Path(root) / 'verified'
            script = Path(root) / 'mark.py'
            script.write_text(f'from pathlib import Path\nPath({str(marker)!r}).touch()\n')
            impl = dict(name='oracle-test', transport='stdio', suites=['test'],
                        command=[sys.executable, str(Path(__file__).resolve().parents[1] / 'loopback.py')],
                        fixtures={'oracle': {'verify': [sys.executable, str(script)]}})
            case = dict(name='bad-wire', fixtures=['oracle'],
                        steps=[{'send': {}}, {'expect': {'wrong': True}}])
            result = asyncio.run(run_case(impl, 'test', case))
            self.assertIsNotNone(result[3])
            self.assertFalse(marker.exists())


if __name__ == '__main__':
    unittest.main()
