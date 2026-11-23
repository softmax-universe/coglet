"""Unit fixtures exercise the gate, not product snapshot evidence."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from PIL import Image

spec = importlib.util.spec_from_file_location('gate', Path(__file__).parents[1] / 'check-design-snapshots.py')
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)

class SnapshotGateTests(unittest.TestCase):
    def test_largest_text_covers_every_text_surface(self):
        keys = gate.expected_keys()
        for theme in ['light', 'dark', 'light-contrast', 'dark-contrast']:
            for surface in ['onboarding-0', 'onboarding-1', 'onboarding-2',
                            'settings-general', 'settings-hotkey', 'settings-audio',
                            'settings-hub', 'settings-coglets', 'settings-updates', 'settings-privacy',
                            'hud-idle', 'hud-listening', 'hud-thinking', 'hud-speaking',
                            'hud-cancelled', 'hud-microphone-denied', 'hud-offline', 'hud-error',
                            'menu-idle', 'menu-listening', 'menu-thinking', 'menu-speaking',
                            'menu-offline', 'menu-update', 'menu-error',
                            'face-idle', 'face-listening', 'face-thinking', 'face-speaking',
                            'face-happy', 'face-concerned', 'about', 'about-licenses']:
                self.assertIn(f'{theme}-{surface}-large', keys)
        self.assertEqual(len(keys), 360)

    def test_missing_baseline_and_manifest_mutation_fail(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            actual, baseline = root / 'actual', root / 'baseline'
            actual.mkdir(); baseline.mkdir()
            (actual / 'manifest.json').write_text('[]')
            self.assertFalse(gate.compare(actual, baseline, root / 'out'))

    def test_changed_pixel_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            actual, baseline = root / 'actual', root / 'baseline'
            actual.mkdir(); baseline.mkdir()
            original = gate.expected_keys
            gate.expected_keys = lambda: {'test-unit-fixture'}
            try:
                (actual / 'manifest.json').write_text(json.dumps(['test-unit-fixture']))
                for directory in (actual, baseline):
                    image = Image.new('RGB', (10, 10), 'white')
                    image.putpixel((0, 0), (0, 0, 0))
                    image.save(directory / 'test-unit-fixture.png')
                self.assertTrue(gate.compare(actual, baseline, root / 'out'))
                Image.new('RGB', (10, 10), 'black').save(actual / 'test-unit-fixture.png')
                self.assertFalse(gate.compare(actual, baseline, root / 'out'))
            finally:
                gate.expected_keys = original

if __name__ == '__main__': unittest.main()
