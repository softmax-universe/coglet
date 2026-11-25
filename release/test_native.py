"""Negative controls for the ELF gate; synthetic headers are unit inputs only."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('native_smoke', Path(__file__).with_name('native_smoke.py'))
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class NativeGateTests(unittest.TestCase):
    def test_elf_dependency_and_abi_refusals(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'unit-elf-header'
            header = bytearray(52)
            header[:6] = b'\x7fELF\x01\x01'
            header[18:20] = (40).to_bytes(2, 'little')
            header[36:40] = (0x400).to_bytes(4, 'little')
            path.write_bytes(header)
            def check(version='GLIBC_2.27 GLIBC_2.4', loader='/lib/ld-linux-armhf.so.3', library='libc.so.6'):
                with patch.object(MODULE.subprocess, 'check_output', side_effect=[version, loader, f'(NEEDED) Shared library: [{library}]']):
                    MODULE.inspect(path)
            check()
            for version in ['GLIBC_2.28', 'GLIBC_2.34', '']:
                with self.assertRaises(ValueError):
                    check(version=version)
            with self.assertRaises(ValueError):
                check(loader='/lib/ld-linux.so.3')
            with self.assertRaises(ValueError):
                check(library='libunknown.so.1')
            header[36:40] = bytes(4)
            path.write_bytes(header)
            with self.assertRaises(ValueError):
                MODULE.inspect(path)

    def test_native_profile_does_not_relabel_other_platforms(self):
        spec = importlib.util.spec_from_file_location('assemble', Path(__file__).with_name('assemble.py'))
        assert spec and spec.loader
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with self.assertRaisesRegex(ValueError, 'requires linux-armv7'):
            module.binary_bundle(Path('/unused'), Path('/unused'), 'v0.0.1-rc1', 'linux-x86_64', native=True)


if __name__ == '__main__':
    unittest.main()
