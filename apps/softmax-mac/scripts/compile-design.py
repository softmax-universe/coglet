#!/usr/bin/env python3
"""Compile design SVGs into native PDF template image sets (uv --with cairosvg)."""
from pathlib import Path
import json
import shutil
import cairosvg

root = Path(__file__).resolve().parents[1]
assets = root / 'Sources/SoftmaxApp/Resources/Assets.xcassets'
assets.mkdir(parents=True, exist_ok=True)
(assets / 'Contents.json').write_text(json.dumps({'info': {'author': 'xcode', 'version': 1}}))
shutil.copytree(root / 'Design/AppIcon.appiconset', assets / 'AppIcon.appiconset', dirs_exist_ok=True)
for source in sorted((root / 'Design/MenuBar').rglob('*18@1x.svg')):
    name = source.stem.replace('-18@1x', '')
    target = assets / f'menu-{name}.imageset'
    target.mkdir(exist_ok=True)
    cairosvg.svg2pdf(url=str(source), write_to=str(target / 'template.pdf'))
    (target / 'Contents.json').write_text(json.dumps({
        'images': [{'idiom': 'universal', 'filename': 'template.pdf'}],
        'info': {'author': 'xcode', 'version': 1},
        'properties': {'preserves-vector-representation': True, 'template-rendering-intent': 'template'}
    }, indent=2) + '\n')
print('Compiled AppIcon and native menu template PDFs.')
