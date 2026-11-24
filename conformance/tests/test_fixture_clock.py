"""Opted-in runner watchdogs must not advance the surface fixture clock."""
import asyncio
from pathlib import Path
import sys

import unittest
from unittest.mock import patch

CONF = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CONF))
from harness import run_case, validate_impl

# Private clock-control socket, not surface protocol. Echoes the actual fixture
# time after an intentionally slow response; ordinary stdio retains real framing.
CHILD = r'''
import os, select, socket, struct, sys, time
s = socket.socket(socket.AF_UNIX)
s.connect(os.environ['COGLET_FIXTURE_CLOCK_SOCKET'])
now = 0
print('READY', file=sys.stderr, flush=True)
while True:
    ready, _, _ = select.select([s, sys.stdin.buffer], [], [])
    if s in ready:
        raw = s.recv(8, socket.MSG_WAITALL)
        if len(raw) != 8: break
        value = int.from_bytes(raw, 'big')
        if value < now: sys.exit(2)
        now = value
        s.sendall(raw)
    if sys.stdin.buffer in ready:
        raw = sys.stdin.buffer.read(4)
        if not raw: break
        body = sys.stdin.buffer.read(int.from_bytes(raw, 'big'))
        time.sleep(.25)
        # canonical CBOR uint32: fixture microseconds, always >65535 here
        out = b'\x1a' + now.to_bytes(4, 'big')
        sys.stdout.buffer.write(len(out).to_bytes(4, 'big') + out)
        sys.stdout.buffer.flush()
        s.sendall(now.to_bytes(8, 'big'))
'''


class FixtureClockTests(unittest.TestCase):
    def impl(self, child=CHILD):
        return dict(name='clock-test', suites=['test'], transport='stdio',
                    command=[sys.executable, '-u', '-c', child],
                    ready_line='READY', fixture_clock=True)

    def test_real_child_slow_io_does_not_advance_fixture_time(self):
        impl = self.impl()
        validate_impl(impl)
        case = dict(name='clock', timeout_ms=4000, steps=[
            {'send': {}, 'at_ms': 100},
            {'expect': 100000, 'within_ms': 200},
            {'send': {}, 'at_ms': 110},
            {'expect': 110000, 'within_ms': 200},
        ])
        result = asyncio.run(run_case(impl, 'test', case))
        self.assertIsNone(result[3], result)

    def test_large_reply_flush_before_send_ack(self):
        # One maximum-size wire frame exceeds both kernel and asyncio pipe buffers.
        child = CHILD.replace("out = b'\\x1a' + now.to_bytes(4, 'big')",
                              "out = b'x' * (1024 * 1024)")
        case = dict(name='large-reply', timeout_ms=5000, steps=[
            {'send': {}, 'at_ms': 100},
            {'expect_hex': (b'x' * (1024 * 1024)).hex()},
        ])
        result = asyncio.run(run_case(self.impl(child), 'test', case))
        self.assertIsNone(result[3], result)

    def test_large_initial_reply_before_clock_connection(self):
        child = CHILD.replace("s = socket.socket(socket.AF_UNIX)",
            "sys.stdout.buffer.write((1024 * 1024).to_bytes(4, 'big') + b'x' * (1024 * 1024)); sys.stdout.buffer.flush()\ns = socket.socket(socket.AF_UNIX)")
        case = dict(name='large-initial', timeout_ms=5000, steps=[
            {'expect_hex': (b'x' * (1024 * 1024)).hex()},
        ])
        result = asyncio.run(run_case(self.impl(child), 'test', case))
        self.assertIsNone(result[3], result)

    def test_stdout_overflow_fails_without_backpressure_deadlock(self):
        child = CHILD.replace("out = b'\\x1a' + now.to_bytes(4, 'big')",
                              "out = b'x' * (5 * 1024 * 1024)")
        case = dict(name='stdout-overflow', timeout_ms=5000, steps=[
            {'send': {}, 'at_ms': 100}, {'expect': {}},
        ])
        result = asyncio.run(run_case(self.impl(child), 'test', case))
        self.assertIn('4 MiB unread byte limit', str(result[3]))
        self.assertNotIn('TimeoutError', str(result[3]))
        self.assertLess(result[2], 2)

    def test_buffered_unexpected_byte_is_not_silence(self):
        case = dict(name='buffered-silence', steps=[
            {'send': {}, 'at_ms': 100}, {'silence_ms': 20},
        ])
        result = asyncio.run(run_case(self.impl(), 'test', case))
        self.assertIn('unexpected bytes during silence', str(result[3]))

    def test_partial_frame_eof_is_failure(self):
        child = CHILD.replace("out = b'\\x1a' + now.to_bytes(4, 'big')", "out = b'x'")
        child = child.replace("len(out).to_bytes(4, 'big')", "(100).to_bytes(4, 'big')")
        child = child.replace("s.sendall(now.to_bytes(8, 'big'))",
                              "s.sendall(now.to_bytes(8, 'big')); os.close(1)")
        case = dict(name='partial-eof', steps=[{'send': {}, 'at_ms': 100}, {'expect': {}}])
        result = asyncio.run(run_case(self.impl(child), 'test', case))
        self.assertIn('IncompleteReadError', str(result[3]))

    def test_clock_requires_stdio_and_exact_boolean(self):
        for options in [dict(fixture_clock=1), dict(transport='unix', socket='unused')]:
            impl = self.impl()
            impl.update(options)
            with self.assertRaises(ValueError):
                validate_impl(impl)

    def test_missing_clock_ack_fails_bounded(self):
        child = CHILD.replace('s.sendall(raw)', 'pass # withhold acknowledgement')
        case = dict(name='missing-clock-ack', timeout_ms=400, steps=[{'send': {}, 'at_ms': 100}])
        result = asyncio.run(run_case(self.impl(child), 'test', case))
        self.assertIsNotNone(result[3])
        self.assertIn('TimeoutError', str(result[3]))
        self.assertLess(result[2], 2)

    def test_eof_is_not_silence(self):
        child = CHILD.replace("print('READY', file=sys.stderr, flush=True)",
                              "print('READY', file=sys.stderr, flush=True); sys.exit(0)")
        case = dict(name='eof', timeout_ms=1000, steps=[{'silence_ms': 20}])
        result = asyncio.run(run_case(self.impl(child), 'test', case))
        self.assertIsNotNone(result[3])


    def test_exit_before_clock_connect_reports_exit(self):
        result = asyncio.run(run_case(self.impl('raise SystemExit(3)'), 'test',
                                      dict(name='early-exit', steps=[{'send': {}}])))
        self.assertIn('exited before fixture clock connection', str(result[3]))
        self.assertLess(result[2], 2)

    def test_inherited_clock_env_is_removed_for_ordinary_adapter(self):
        child = "import os,sys; assert 'COGLET_FIXTURE_CLOCK_SOCKET' not in os.environ; sys.stdout.buffer.write(bytes.fromhex('00000001a0')); sys.stdout.buffer.flush()"
        impl = self.impl(child)
        impl.pop('fixture_clock')
        impl.pop('ready_line')
        with patch.dict('os.environ', {'COGLET_FIXTURE_CLOCK_SOCKET': 'foreign'}):
            result = asyncio.run(run_case(impl, 'test', dict(name='ordinary-env', steps=[{'expect': {}}])))
        self.assertIsNone(result[3], result)

    def test_step_watchdog_fails_missing_ack_without_total_case_expiry(self):
        child = CHILD.replace('s.sendall(raw)', 'pass # withhold acknowledgement')
        case = dict(name='step-watchdog', timeout_ms=8000, steps=[{'send': {}, 'at_ms': 100}])
        result = asyncio.run(run_case(self.impl(child), 'test', case))
        self.assertIn('step 0: TimeoutError', str(result[3]))
        self.assertLess(result[2], 5)

    def test_incorrect_clock_ack_is_failure(self):
        child = CHILD.replace('s.sendall(raw)', "s.sendall((0).to_bytes(8, 'big'))")
        case = dict(name='bad-ack', steps=[{'send': {}, 'at_ms': 100}])
        result = asyncio.run(run_case(self.impl(child), 'test', case))
        self.assertIn('acknowledgement mismatch', str(result[3]))

    def test_unexpected_response_is_not_filtered(self):
        case = dict(name='wrong-frame', steps=[
            {'send': {}, 'at_ms': 100}, {'expect': {'type': 'not-a-clock'}}])
        result = asyncio.run(run_case(self.impl(), 'test', case))
        self.assertIn('expected', str(result[3]))

    def test_without_clock_the_old_wall_deadline_still_fails(self):
        child = "import time; time.sleep(.3); print('late', flush=True); time.sleep(1)"
        impl = dict(name='ordinary', suites=['test'], transport='stdio',
                    command=[sys.executable, '-u', '-c', child])
        case = dict(name='wall-budget', steps=[{'expect': {}, 'within_ms': 200}])
        result = asyncio.run(run_case(impl, 'test', case))
        self.assertIn('step 0: TimeoutError', str(result[3]))


if __name__ == '__main__':
    unittest.main()
