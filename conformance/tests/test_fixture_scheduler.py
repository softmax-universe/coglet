"""ADR-0024 cross-channel barriers and invalid scheduler peers fail closed."""
import asyncio
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from harness import BufferedOutput, Peer, validate_impl


class SchedulerTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.peer = Peer(dict(transport='stdio', fixture_clock=True, fixture_scheduler=True))
        self.source = asyncio.StreamReader()
        self.peer.reader = BufferedOutput(self.source)
        self.peer.clock_reader = asyncio.StreamReader()

    async def asyncTearDown(self):
        self.source.feed_eof()
        await self.peer.reader.task

    def ack(self, value):
        self.peer.clock_reader.feed_data(json.dumps(value).encode() + b'\n')

    async def test_ack_waits_for_wire_watermark_not_control_delivery(self):
        self.ack({'time': 10, 'bytes': 4})
        entered = asyncio.Event()
        original = self.peer.reader.wait_received
        async def wait(total):
            entered.set()
            await original(total)
        self.peer.reader.wait_received = wait
        task = asyncio.create_task(self.peer.scheduler_ack(10, 10))
        try:
            await asyncio.wait_for(entered.wait(), 1)
            self.assertFalse(task.done())
            self.source.feed_data(b'abcd')
            await asyncio.wait_for(task, 1)
            self.assertEqual(await self.peer.reader.readexactly(4), b'abcd')
        finally:
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)

    async def test_missing_wire_bytes_with_eof_is_not_an_ack(self):
        self.ack({'time': 0, 'bytes': 4})
        self.source.feed_eof()
        with self.assertRaisesRegex(EOFError, 'byte watermark'):
            await self.peer.scheduler_ack(0, 0)

    async def test_invalid_acknowledgements(self):
        for value in [None, {}, {'time': True, 'bytes': 0}, {'time': -1, 'bytes': 0},
                      {'time': 11, 'bytes': 0}, {'time': 0, 'bytes': -1},
                      {'time': 0, 'bytes': False}, {'time': 0, 'bytes': 0, 'extra': 1}]:
            with self.subTest(value=value):
                self.ack(value)
                with self.assertRaisesRegex(ValueError, 'invalid scheduler acknowledgement'):
                    await self.peer.scheduler_ack(0, 10)

    async def test_byte_watermark_cannot_roll_back(self):
        self.source.feed_data(b'abc')
        self.ack({'time': 0, 'bytes': 3})
        await self.peer.scheduler_ack(0, 0)
        self.ack({'time': 0, 'bytes': 2})
        with self.assertRaisesRegex(ValueError, 'invalid scheduler acknowledgement'):
            await self.peer.scheduler_ack(0, 0)

    async def test_eof_and_logical_timeout_are_failures(self):
        with self.assertRaisesRegex(TimeoutError, 'logical fixture receive deadline'):
            await self.peer.scheduled_receive(0)
        self.source.feed_eof()
        await self.peer.reader.task
        with self.assertRaisesRegex(EOFError, 'fixture output ended'):
            await self.peer.scheduled_receive(200)

    async def test_registry_requires_explicit_clock_and_boolean(self):
        base = dict(name='test', suites=['test'], transport='stdio', command=['unused'])
        for options in [dict(fixture_scheduler=True),
                        dict(fixture_clock=True, fixture_scheduler=1),
                        dict(fixture_clock=True, fixture_scheduler=True, transport='unix', socket='unused')]:
            with self.subTest(options=options), self.assertRaises(ValueError):
                validate_impl(dict(base, **options))
        validate_impl(dict(base, fixture_clock=True, fixture_scheduler=True))


if __name__ == '__main__':
    unittest.main()
