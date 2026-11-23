"""Native compilation contract, independent of Cairo and AppKit availability."""
import json
import re
import unittest
import zlib
from pathlib import Path

ASSETS = Path(__file__).resolve().parents[2] / 'Sources/SoftmaxApp/Resources/Assets.xcassets'

class NativeAssetTests(unittest.TestCase):
    def test_all_template_pdf_pages_are_eighteen_points(self):
        names = {'idle', 'listening', 'thinking', 'speaking', 'offline', 'update', 'error'}
        names.update(f'{state}-{frame}' for state in ['listening', 'speaking'] for frame in range(4))
        actual = {p.name.removeprefix('menu-').removesuffix('.imageset') for p in ASSETS.glob('menu-*.imageset')}
        self.assertEqual(actual, names)
        for name in sorted(names):
            with self.subTest(name=name):
                directory = ASSETS / f'menu-{name}.imageset'
                metadata = json.loads((directory / 'Contents.json').read_text())
                self.assertEqual(metadata['properties']['template-rendering-intent'], 'template')
                data = (directory / 'template.pdf').read_bytes()
                # Cairo PDF 1.7 may place the page dictionary in a Flate object stream.
                dictionaries = data
                for stream in re.findall(rb'stream\r?\n(.*?)\r?\nendstream', data, re.S):
                    try:
                        dictionaries += b'\n' + zlib.decompress(stream)
                    except zlib.error:
                        pass
                box = re.search(rb'/MediaBox\s*\[([^]]+)\]', dictionaries)
                if box is None:
                    self.fail('Cairo output has no readable PDF page MediaBox')
                self.assertEqual([float(value) for value in box[1].split()], [0, 0, 18, 18])

if __name__ == '__main__':
    unittest.main()
