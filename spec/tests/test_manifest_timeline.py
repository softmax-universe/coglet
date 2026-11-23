"""Execute manifest-admission case contracts; not runtime conformance."""
import asyncio
import copy
import unittest
from types import SimpleNamespace

from build_manifest_admission import cases
from test_abi import check_manifest_admission
from test_surface_case_timeline import Writer, harness
from wire import encode


class ManifestTimeline(unittest.IsolatedAsyncioTestCase):
    async def run_case(self, case, corrupt=False):
        request = case['steps'][0]['send']
        try:
            check_manifest_admission(request['manifest'], request['catalog'], request['grant'])
            response = dict(admitted=True, reason=None, instantiated=False)
        except ValueError as error:
            response = dict(admitted=False, reason=str(error), instantiated=False)
        if corrupt:
            response['admitted'] = not response['admitted']
        peer = SimpleNamespace(impl={'transport': 'stdio'}, writer=Writer(), reader=asyncio.StreamReader())
        raw = encode(response)
        peer.reader.feed_data(len(raw).to_bytes(4, 'big') + raw)
        peer.reader.feed_eof()
        await harness.timeline(peer, copy.deepcopy(case))
        self.assertEqual(await peer.reader.read(), b'')
        self.assertTrue(peer.writer.frames)

    async def test_admission_case_contracts(self):
        for case in cases():
            with self.subTest(case=case['name']):
                await self.run_case(case)
                with self.assertRaises(AssertionError):
                    await self.run_case(case, corrupt=True)
