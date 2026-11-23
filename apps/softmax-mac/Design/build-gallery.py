#!/usr/bin/env python3
"""Build a portable screenshot index and normal-size review contact sheets."""
from pathlib import Path
import html
from PIL import Image, ImageDraw
ROOT=Path(__file__).resolve().parent
shots=ROOT/'mockups/screenshots'
files=sorted(p for p in shots.glob('*.jpg') if not p.name.endswith('-overview.jpg'))+sorted(shots.glob('*proof.png'))
items=''.join(f'<figure><a href="screenshots/{p.name}"><img loading="lazy" src="screenshots/{p.name}" alt="{html.escape(p.stem)}"></a><figcaption>{html.escape(p.stem)}</figcaption></figure>' for p in files)
(ROOT/'mockups/screenshots.html').write_text('<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Softmax design screenshots</title><style>body{font:14px Arial,sans-serif;background:#f4f7f5;color:#17231d;margin:24px}main{display:grid;grid-template-columns:repeat(auto-fit,minmax(300px,1fr));gap:24px}figure{margin:0}img{width:100%;border:1px solid #d0dbd4}figcaption{padding:8px 0}a{color:#176842}</style><h1>Softmax for Mac · rendered prototype states</h1><p>Browser renders, not native AppKit captures. All content is sample. Click a thumbnail for the full image.</p><p><a href="softmax-mac.html">Open interactive prototype</a></p><main>'+items+'</main></html>\n')
for theme in ['light','dark']:
    canvas=Image.new('RGB',(1440,1490),'#f4f7f5' if theme=='light' else '#111714')
    draw=ImageDraw.Draw(canvas)
    cases=[('menu','idle'),('face','listening'),('hud','listening'),('settings','general'),('onboarding','microphone'),('about','default')]
    for i,(view,state) in enumerate(cases):
        im=Image.open(shots/f'{theme}-{view}-{state}.jpg')
        im.thumbnail((696,410))
        x,y=(i%2)*720+12,(i//2)*470+42
        draw.text((x,y-30),f'{view.upper()} · {state}',fill='#17231d' if theme=='light' else '#ecf5ef',font_size=18)
        canvas.paste(im,(x,y))
    canvas.save(shots/f'{theme}-overview.jpg',quality=90)
print(f'Indexed {len(files)} captures; wrote two overview sheets.')
