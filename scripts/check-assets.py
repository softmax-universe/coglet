#!/usr/bin/env python3
"""Dependency-free asset contract checks, including every PNG size and SVG alpha source."""
import json
import struct
import sys
import xml.etree.ElementTree as ET
import zlib
from pathlib import Path

ROOT = Path(sys.argv[1]) if len(sys.argv)>1 else Path(__file__).resolve().parents[1]/'apps/softmax-mac/Design'
checks=0

def require(ok, message):
    global checks
    assert ok, message
    checks+=1

def png_size(path):
    data=path.read_bytes()
    require(data[:8]==b'\x89PNG\r\n\x1a\n',f'{path}: PNG signature')
    offset=8
    chunks=[]
    while offset<len(data):
        size=struct.unpack('>I',data[offset:offset+4])[0]
        kind=data[offset+4:offset+8]
        chunk=data[offset+8:offset+8+size]
        crc=struct.unpack('>I',data[offset+8+size:offset+12+size])[0]
        require(zlib.crc32(kind+chunk)&0xffffffff==crc,f'{path}: CRC')
        chunks.append(kind)
        offset+=size+12
    require(chunks[0]==b'IHDR' and chunks[-1]==b'IEND' and b'IDAT' in chunks,f'{path}: complete PNG')
    return struct.unpack('>II',data[16:24])

try:
    manifest=json.loads((ROOT/'AppIcon.appiconset/Contents.json').read_text())
    expected={(f'{s}x{s}',f'{scale}x') for s in [16,32,128,256,512] for scale in [1,2]}
    images=manifest['images']
    require(len(images)==10,'Exactly ten macOS icon slots')
    require({(x['size'],x['scale']) for x in images}==expected,'Every required macOS icon size and scale')
    require(len({x['filename'] for x in images})==10,'Unique icon files')
    for image in images:
        require(image['idiom']=='mac','mac idiom')
        pixels=int(image['size'].split('x')[0])*int(image['scale'][0])
        require(png_size(ROOT/'AppIcon.appiconset'/image['filename'])==(pixels,pixels),f'{image}: pixel size')
    master=ET.parse(ROOT/'AppIcon.svg').getroot()
    require(master.attrib['viewBox']=='0 0 1024 1024','1024 master SVG')
    for state in ['idle','listening','thinking','speaking','offline','update','error']:
        for pt in [16,18]:
            for scale in [1,2]:
                p=ROOT/f'MenuBar/{state}-{pt}@{scale}x.svg'
                svg=ET.parse(p).getroot()
                require(int(svg.attrib['width'])==pt*scale and int(svg.attrib['height'])==pt*scale,f'{p}: correct size')
                require(svg.attrib['viewBox']==f'0 0 {pt*scale} {pt*scale}',f'{p}: viewBox')
                for node in svg.iter():
                    for key in ['fill','stroke']:
                        require(node.attrib.get(key,'#000') in ['#000','none'],f'{p}: monochrome only')
                    require('href' not in node.attrib and 'style' not in node.attrib,f'{p}: no remote assets or inline styles')
    animation=json.loads((ROOT/'MenuBar/animations.json').read_text())
    require(animation['frameDurationMs']==160,'Animation timing')
    for state in ['listening','speaking']:
        frames=animation['states'][state]
        require(len(frames)==4,f'{state}: four frames')
        for frame in frames:
            require(len(frame)==4,'four size/scale variants per frame')
            for path in frame:
                ET.parse(ROOT/path)
                checks+=1
        require(len({(ROOT/frame[0]).read_bytes() for frame in frames})>1,f'{state}: frames change')
    for scale in [1,2]:
        require(png_size(ROOT/f'DMG/background@{scale}x.png')==(660*scale,440*scale),'DMG dimensions')
    for name in ['welcome','permissions','hub']:
        ET.parse(ROOT/f'Onboarding/{name}.svg');checks+=1
    for state in ['idle','listening','thinking','speaking','happy','concerned','sleeping']:
        ids={n.attrib.get('id') for n in ET.parse(ROOT/f'Face/{state}.svg').iter()}
        require({'head','eyes','mouth'}<=ids,f'{state}: layered Face')
    tokens=json.loads((ROOT/'DesignTokens.json').read_text())
    swift=(ROOT/'DesignTokens.swift').read_text()
    for group in ['font','radius','motion']:
        for name,value in tokens[group].items():
            if isinstance(value,(float,int)):
                require(f'static let {group}{name[0].upper()+name[1:]}: CGFloat = {value}\n' in swift,f'Swift token {group}.{name}')
    require('['+', '.join(map(str,tokens['space']))+']' in swift,'Swift spacing')
    for theme,colors in tokens['color'].items():
        for name,value in colors.items():
            rgb=[int(value[i:i+2],16)/255 for i in [1,3,5]]
            line=f'static let {name} = Color(.sRGB, red: {rgb[0]:.9f}, green: {rgb[1]:.9f}, blue: {rgb[2]:.9f}, opacity: 1)'
            require(line in swift,f'Swift color {theme}.{name}')
    def lum(c):
        values=[int(c[i:i+2],16)/255 for i in [1,3,5]]
        return sum(w*(v/12.92 if v<=.04045 else ((v+.055)/1.055)**2.4) for w,v in zip([.2126,.7152,.0722],values))
    for theme,c in tokens['color'].items():
        for fg,bg in [('ink','surface'),('muted','surface'),('muted','background'),('accentInk','accent'),('danger','surface')]:
            a,b=sorted([lum(c[fg]),lum(c[bg])]);ratio=(b+.05)/(a+.05)
            require(ratio>=4.5,f'{theme} {fg}/{bg} contrast {ratio:.2f}')
            print(f'contrast {theme} {fg}/{bg}: {ratio:.2f}:1')
    require((ROOT/'mockups/softmax-mac.html').is_file(),'Clickable prototype')
    print(f'PASS: {checks} asset contract checks; ten icon slots; 28 static templates; 32 animation SVGs.')
except (AssertionError, OSError, ValueError, KeyError, ET.ParseError, struct.error) as e:
    print(f'FAIL: {e}',file=sys.stderr)
    sys.exit(1)
