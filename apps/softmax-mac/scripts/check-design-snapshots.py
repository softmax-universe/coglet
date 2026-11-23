#!/usr/bin/env python3
"""Strict native snapshot gate; missing/extra baselines fail (never auto-record in CI).
Run: uv run --with pillow scripts/check-design-snapshots.py --actual build/DesignSnapshots
Reviewed macOS reference PNGs live in Tests/DesignSnapshots. Browser mockups are
comparison context only, never accepted as native references.
"""
import argparse
import json
from pathlib import Path
from PIL import Image, ImageChops, ImageDraw, ImageStat

ROOT = Path(__file__).resolve().parents[1]

def expected_keys():
    names = [f'onboarding-{i}' for i in range(3)]
    names += ['onboarding-voice-' + s for s in ['notDetermined', 'restricted', 'denied', 'granted', 'notGranted', 'unknown']]
    names += ['onboarding-login-' + s for s in ['disabled', 'enabled', 'requiresApproval', 'unavailable', 'unknown']]
    for tab in ['general', 'hotkey', 'audio', 'hub', 'coglets', 'updates', 'privacy']:
        names += [f'settings-{tab}', f'settings-{tab}-large']
    names += ['hud-' + s for s in ['idle', 'listening', 'thinking', 'speaking', 'cancelled', 'microphone-denied', 'offline', 'error']]
    for state in ['idle', 'listening', 'thinking', 'speaking', 'offline', 'update', 'error']:
        names += ['menu-' + state, 'template-' + state]
    names += ['face-' + s for s in ['idle', 'listening', 'thinking', 'speaking', 'happy', 'concerned']]
    names += ['about', 'about-licenses', 'commands', 'commands-filtered', 'commands-empty']
    # Settings already have largest-text coverage. Exercise every other text surface
    # at 1.6x, not just its ordinary-size bitmap at a larger capture geometry.
    names += [n + '-large' for n in list(names)
              if not n.startswith(('settings-', 'template-', 'onboarding-voice-', 'onboarding-login-'))]
    return {theme + '-' + n for theme in ['light', 'dark', 'light-contrast', 'dark-contrast'] for n in names}

def compare(actual, baseline, output):
    keys = expected_keys()
    manifest = json.loads((actual / 'manifest.json').read_text())
    errors = []
    if len(manifest) != len(set(manifest)) or set(manifest) != keys:
        errors.append('Native manifest does not match explicit surface/state matrix')
    if {p.stem for p in baseline.glob('*.png')} != keys:
        errors.append('Baseline coverage differs from explicit surface/state matrix')
    output.mkdir(parents=True, exist_ok=True)
    for key in sorted(keys):
        if not (actual / (key + '.png')).exists():
            errors.append(f'{key}: missing actual'); continue
        new = Image.open(actual / (key + '.png')).convert('RGB')
        if max(ImageStat.Stat(new).var) < 0.5:
            errors.append(f'{key}: blank or uniform native render')
        if (baseline / (key + '.png')).exists():
            old = Image.open(baseline / (key + '.png')).convert('RGB')
            if old.size != new.size:
                errors.append(f'{key}: size {old.size} != {new.size}')
            else:
                diff = ImageChops.difference(old, new)
                changed = sum(max(pixel) > 10 for pixel in diff.getdata()) / (new.width * new.height)
                if changed > 0.002:
                    errors.append(f'{key}: {changed:.3%} pixels changed (limit 0.2%)')
                    diff.save(output / (key + '-diff.png'))
        else:
            errors.append(f'{key}: missing reviewed baseline')
        # Side by side native / HTML. Different geometry is intentional and labelled;
        # no resizing native to masquerade as a matching screenshot.
        mock_key = key.replace('onboarding-0', 'onboarding-welcome').replace('onboarding-1', 'onboarding-microphone').replace('onboarding-2', 'onboarding-ready')
        if mock_key.endswith('-about'): mock_key += '-default'
        if mock_key.endswith('-settings-hub'): mock_key += '-account'
        mock = ROOT / 'Design/mockups/screenshots' / (mock_key + '.jpg')
        if mock.exists():
            reference = Image.open(mock).convert('RGB')
            sheet = Image.new('RGB', (new.width + reference.width + 24, max(new.height, reference.height) + 48), '#dddddd')
            draw = ImageDraw.Draw(sheet)
            draw.text((8, 8), 'NATIVE — ' + key, fill='black')
            draw.text((new.width + 24, 8), 'HTML DESIGN REFERENCE', fill='black')
            sheet.paste(new, (0, 40)); sheet.paste(reference, (new.width + 24, 40))
            sheet.save(output / (key + '-comparison.jpg'))
    report = {'expected': len(keys), 'actual': len(manifest), 'errors': errors}
    (output / 'report.json').write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))
    return not errors

if __name__ == '__main__':
    p = argparse.ArgumentParser()
    p.add_argument('--actual', type=Path, required=True)
    p.add_argument('--baseline', type=Path, default=ROOT / 'Tests/DesignSnapshots')
    p.add_argument('--output', type=Path, default=ROOT / 'build/DesignComparisons')
    a = p.parse_args()
    raise SystemExit(0 if compare(a.actual, a.baseline, a.output) else 1)
