"""Startup has a bounded connection budget, not a protocol response budget."""
import asyncio
from pathlib import Path
import sys
import unittest

CONF = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CONF))
from harness import Peer, run_case, validate_impl

MARKER = 'COGLET_CONFORMANCE_READY'


class ReadinessTests(unittest.TestCase):
    def run_adapter(self, source, **options):
        impl = dict(name='ready-test', suites=['test'], transport='stdio',
                    command=[sys.executable, '-c', source], ready_line=MARKER,
                    connect_timeout_ms=2000)
        impl.update(options)
        case = dict(name='hello', timeout_ms=3000,
                    steps=[dict(expect={}, within_ms=200)])
        return asyncio.run(run_case(impl, 'test', case))

    def test_slow_startup_does_not_spend_first_frame_budget(self):
        result = self.run_adapter("import sys,time; time.sleep(.3); "
                                  f"print('{MARKER}',file=sys.stderr,flush=True); "
                                  "sys.stdout.buffer.write(b'\\0\\0\\0\\1\\xa0'); sys.stdout.flush(); "
                                  "sys.stdin.buffer.read()")
        self.assertIsNone(result[3], result)

    def test_post_ready_response_still_times_out(self):
        result = self.run_adapter("import sys,time; "
                                  f"print('{MARKER}',file=sys.stderr,flush=True); "
                                  "time.sleep(.5); sys.stdout.buffer.write(b'\\0\\0\\0\\1\\xa0'); "
                                  "sys.stdout.flush(); sys.stdin.buffer.read()")
        self.assertIn('step 0: TimeoutError', result[3])

    def test_missing_signal_exits_fail_closed_with_stderr(self):
        result = self.run_adapter("import sys; print('startup failed',file=sys.stderr)")
        self.assertIn('adapter exited before readiness signal', result[3])
        self.assertIn('startup failed', result[3])

    def test_connect_watchdog_bounds_missing_signal(self):
        result = self.run_adapter('import time; time.sleep(10)', connect_timeout_ms=100)
        self.assertEqual(result[3], 'TimeoutError: ')

    def test_case_watchdog_still_includes_startup(self):
        result = self.run_adapter('import time; time.sleep(10)', connect_timeout_ms=5000)
        self.assertEqual(result[3], 'TimeoutError: ')

    def test_marker_is_exact_line_and_can_be_fragmented(self):
        async def check(chunks, succeeds):
            class Reader:
                async def read(self, _):
                    return chunks.pop(0) if chunks else b''
            class Process:
                stderr = Reader()
            peer = Peer(dict(ready_line=MARKER))
            peer.process = Process()
            peer.ready = asyncio.get_running_loop().create_future()
            await peer.drain_stderr()
            if succeeds:
                self.assertIsNone(await asyncio.wait_for(peer.ready, .5))
            else:
                with self.assertRaisesRegex(RuntimeError, 'before readiness'):
                    await asyncio.wait_for(peer.ready, .5)
            self.assertLessEqual(len(peer.stderr), 8192)
        asyncio.run(check([b'noise\nCOGLET_', b'CONFORMANCE_READY', b'\n'], True))
        for chunks in [[MARKER.encode()], [b'prefix '+MARKER.encode()+b'\n'],
                       [b'x'*9000, MARKER.encode()+b'\n']]:
            asyncio.run(check(chunks, False))

    def test_ready_line_validation(self):
        base = dict(name='ready-test', suites=['test'], transport='stdio', command=['adapter'])
        validate_impl(dict(base, ready_line=MARKER))
        for marker in ['', 'a\nb', 'é', 'x'*129, True]:
            with self.subTest(marker=marker), self.assertRaises(ValueError):
                validate_impl(dict(base, ready_line=marker))
        with self.assertRaises(ValueError):
            validate_impl(dict(base, transport='unix', socket='test.sock', ready_line=MARKER))


if __name__ == '__main__':
    unittest.main()
