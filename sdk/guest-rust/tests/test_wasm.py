"""Execute compiled guests against a restricted WASI/ABI host, not a runtime oracle.

This is supplemental guest export qualification; it does not claim that the
runtime mock or WAMR has passed conformance/abi's journal timelines.
"""
import json
import pathlib
import sys
import tempfile
import unittest
import wasmtime

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from conformance.wire import encode, decode


def event():
    return dict(source='surface:' + '1' * 64, kind='test.echo', seq=0,
                logical_time=42, payload={'text': 'hello', 'u64': 2**64-1},
                correlation_id='2' * 32)


class Guest:
    def __init__(self, name, directory, result=0):
        self.calls = []
        self.engine = wasmtime.Engine()
        self.store = wasmtime.Store(self.engine)
        self.module = wasmtime.Module.from_file(self.engine, str(
            ROOT / 'target/guests/wasm32-wasip1/release' / f'coglet_{name}.wasm'))
        config = wasmtime.WasiConfig()
        for mount in ('code', 'data', 'logs'):
            local = pathlib.Path(directory) / mount
            local.mkdir(exist_ok=True)
            config.preopen_dir(str(local), '/' + mount)
        self.store.set_wasi(config)
        linker = wasmtime.Linker(self.engine)
        linker.define_wasi()
        linker.allow_shadowing = True
        for imp in self.module.imports:
            if imp.module == 'coglet_v0':
                def call(caller, ptr, size, op=imp.name):
                    memory = caller.get('memory')
                    ptr &= 0xffffffff; size &= 0xffffffff
                    if ptr + size > memory.data_len(caller):
                        raise wasmtime.Trap('out of range ABI argument')
                    raw = bytes(memory.read(caller, ptr, ptr + size))
                    self.calls.append((op, decode(raw), raw))
                    return result
                linker.define_func(imp.module, imp.name, imp.type, call, access_caller=True)
            elif imp.name in ('clock_time_get', 'random_get', 'poll_oneoff'):
                def forbidden(*args, name=imp.name):
                    raise wasmtime.Trap('forbidden ambient import: ' + name)
                linker.define_func(imp.module, imp.name, imp.type, forbidden)
        self.instance = linker.instantiate(self.store, self.module)
        self.exports = self.instance.exports(self.store)
        if '_initialize' in self.exports:
            self.exports['_initialize'](self.store)

    def wake(self, events, step=0, time=42, seed=7):
        raw = encode(events)
        ptr = self.exports['abi_alloc'](self.store, len(raw)) & 0xffffffff
        self.exports['memory'].write(self.store, raw, ptr)
        # Python binding accepts signed values for the unsigned Wasm bit patterns.
        def signed(v): return v if v < 2**63 else v - 2**64
        return self.exports['handle'](self.store, ptr, len(raw), signed(step), signed(time), signed(seed))


class WasmContract(unittest.TestCase):
    def test_no_forbidden_imports(self):
        for name in ('echo', 'counter'):
            with self.subTest(guest=name), tempfile.TemporaryDirectory() as directory:
                guest = Guest(name, directory)
                forbidden = {'clock_time_get', 'random_get', 'poll_oneoff', 'sock_accept', 'sock_recv', 'sock_send', 'sock_shutdown'}
                self.assertFalse(forbidden.intersection(i.name for i in guest.module.imports))

    def test_manifests_match_frozen_schema_and_declared_memory(self):
        from jsonschema import Draft202012Validator
        from referencing import Registry, Resource
        documents = [json.loads(path.read_text()) for path in (ROOT / 'spec').rglob('*.schema.json')]
        registry = Registry().with_resources((s['$id'], Resource.from_contents(s)) for s in documents if '$id' in s)
        validator = Draft202012Validator(json.loads((ROOT / 'spec/manifest.schema.json').read_text()), registry=registry)
        for name in ('echo', 'counter'):
            manifest = json.loads((ROOT / f'coglets/{name}/manifest.json').read_text())
            validator.validate(manifest)
            self.assertEqual(len(manifest['subscriptions']), len(set(manifest['subscriptions'])))
            with tempfile.TemporaryDirectory() as directory:
                guest = Guest(name, directory)
                limits = guest.exports['memory'].type(guest.store).limits
                self.assertLessEqual(limits.min, manifest['budgets']['memory_pages'])
                self.assertEqual(limits.max, manifest['budgets']['memory_pages'])

    def test_echo_executes_real_wasm_and_preserves_payload(self):
        with tempfile.TemporaryDirectory() as directory:
            guest = Guest('echo', directory)
            self.assertEqual(guest.wake([event()]), 0)
            self.assertEqual(len(guest.calls), 1)
            op, action, raw = guest.calls[0]
            self.assertEqual(op, 'emit')
            self.assertEqual(action['args'], event()['payload'])
            self.assertEqual(action['correlation_id'], event()['correlation_id'])
            self.assertEqual(encode(action), raw)
            self.assertIsNotNone(guest.exports['memory'].type(guest.store).limits.max)
            self.assertNotIn('_start', guest.exports)

    def test_counter_persists_between_fresh_instances(self):
        with tempfile.TemporaryDirectory() as directory:
            for step in (0, 1):
                guest = Guest('counter', directory)
                self.assertEqual(guest.wake([event()], step=step), 0)
                self.assertEqual((pathlib.Path(directory) / 'data/count').read_text(), str(step + 1))
                self.assertEqual(guest.calls[0][0], 'log')
                self.assertEqual(guest.calls[0][1]['message'], f'count={step + 1}')

    def test_echo_host_rejection_aborts_guest(self):
        with tempfile.TemporaryDirectory() as directory:
            guest = Guest('echo', directory, result=2)
            self.assertNotEqual(guest.wake([event()]), 0)

    def test_invalid_pointer_and_malformed_envelope_do_not_call_host(self):
        with tempfile.TemporaryDirectory() as directory:
            guest = Guest('echo', directory)
            self.assertNotEqual(guest.exports['handle'](guest.store, -1, 100, 0, 0, 0), 0)
            bad = event(); bad['unknown'] = True
            self.assertNotEqual(guest.wake([bad]), 0)
            self.assertEqual(guest.calls, [])

    def test_multiple_allocations_remain_live_until_handle_returns(self):
        with tempfile.TemporaryDirectory() as directory:
            guest = Guest('echo', directory)
            raw = encode([event()])
            ptr = guest.exports['abi_alloc'](guest.store, len(raw)) & 0xffffffff
            guest.exports['memory'].write(guest.store, raw, ptr)
            guest.exports['abi_alloc'](guest.store, 16)
            self.assertEqual(guest.exports['handle'](guest.store, ptr, len(raw), -1, 42, -1), 0)
            self.assertEqual(len(guest.calls), 1)

    def test_counter_refuses_corrupt_state_and_overflow(self):
        for text in ('invalid', str(2**64-1)):
            with self.subTest(text=text), tempfile.TemporaryDirectory() as directory:
                data = pathlib.Path(directory) / 'data'
                data.mkdir(); (data / 'count').write_text(text)
                guest = Guest('counter', directory)
                self.assertNotEqual(guest.wake([event()]), 0)
                self.assertEqual((data / 'count').read_text(), text)
                self.assertEqual(guest.calls, [])

    def test_empty_batch_aborts_without_effects(self):
        with tempfile.TemporaryDirectory() as directory:
            guest = Guest('echo', directory)
            self.assertNotEqual(guest.wake([]), 0)
            self.assertEqual(guest.calls, [])


if __name__ == '__main__':
    unittest.main()
