#!/usr/bin/env python3
"""Rebuild committed exports: uv run --with cairosvg --with pillow generate-assets.py."""
from pathlib import Path
import json
import math
import cairosvg
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent
STATES = ['idle', 'listening', 'thinking', 'speaking', 'offline', 'update', 'error']

def save(name, value):
    p = ROOT / name
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(value + '\n')

def svg(body, w=1024, h=None):
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h or w}" viewBox="0 0 {w} {h or w}">{body}</svg>'

def png(source, target, width, height=None):
    p = ROOT / target
    p.parent.mkdir(parents=True, exist_ok=True)
    cairosvg.svg2png(bytestring=source.encode(), write_to=str(p), output_width=width, output_height=height or width)

def face(state='idle', color='#87EFBC', frame=0):
    eye_height = 80 if state != 'thinking' else 26
    mouth = 40 + (frame % 3) * 12 if state == 'speaking' else 16
    return f'<g id="head"><rect x="212" y="212" width="600" height="600" rx="260" fill="{color}"/></g><g id="eyes" fill="#101917"><rect x="360" y="390" width="58" height="{eye_height}" rx="29"/><rect x="606" y="390" width="58" height="{eye_height}" rx="29"/></g><g id="mouth"><rect x="428" y="570" width="168" height="{mouth}" rx="{mouth/2}" fill="#101917"/></g>'

master = svg('<rect x="56" y="56" width="912" height="912" rx="206" fill="#101917"/>' + face())
save('AppIcon.svg', master)
entries = []
for size in [16, 32, 128, 256, 512]:
    for scale in [1, 2]:
        filename = f'icon-{size}@{scale}x.png'
        png(master, f'AppIcon.appiconset/{filename}', size * scale)
        entries.append({'idiom':'mac', 'size':f'{size}x{size}', 'scale':f'{scale}x', 'filename':filename})
save('AppIcon.appiconset/Contents.json', json.dumps({'images':entries,'info':{'author':'xcode','version':1}}, indent=2))

# Eight flat glyph explorations, not eight shipped brands.
concepts = [face(), '<path d="M280 650V370h120v170h224V370h120v280Z" fill="#87EFBC"/>', '<path d="M290 340h444v100H400v90h280v100H400v100H290Z" fill="#87EFBC"/>', '<circle cx="512" cy="512" r="230" fill="none" stroke="#87EFBC" stroke-width="85"/><circle cx="512" cy="512" r="65" fill="#87EFBC"/>', '<path d="M310 660V370l202 170 202-170v290" fill="none" stroke="#87EFBC" stroke-width="95" stroke-linejoin="round"/>', '<path d="M320 660V450q192-240 384 0v210H580V480q-68-80-136 0v180Z" fill="#87EFBC"/>', '<path d="M300 470h424v170H470l-130 90v-90h-40Z" fill="#87EFBC"/>', '<path d="M300 360h150v300H300Zm274 0h150v300H574Z" fill="#87EFBC"/>']
names = ['Companion', 'Dock', 'Fold', 'Orbit', 'Duet', 'Arch', 'Reply', 'Pause']
sheet = Image.new('RGB', (1280, 720), '#f4f6f5')
draw = ImageDraw.Draw(sheet)
for i, body in enumerate(concepts):
    source = svg('<rect x="56" y="56" width="912" height="912" rx="206" fill="#101917"/>' + body)
    save(f'concepts/{i+1}-{names[i].lower()}.svg', source)
    x, y = (i % 4)*320, (i // 4)*360
    for size, dx in [(180, 0), (60, 194), (29, 264)]:
        import io
        im = Image.open(io.BytesIO(cairosvg.svg2png(bytestring=source.encode(), output_width=size, output_height=size)))
        sheet.paste(im, (x+dx, y+32), im)
    draw.text((x+12,y+245),f'{i+1}. {names[i]}' + (' / recommended' if i == 0 else ''),fill='#101917',font_size=18)
sheet.save(ROOT/'concepts/contact-sheet.png')

# NSImage.isTemplate = true; black fill only, transparent background.
def menu(state, frame=0):
    body = '<rect x="3" y="4" width="12" height="10" rx="5" fill="none" stroke="#000" stroke-width="1.5"/>'
    if state == 'listening':
        body += ''.join(f'<rect x="{6+j*2.5}" y="{7-(j+frame)%2}" width="1" height="{4+2*((j+frame)%2)}" rx=".5"/>' for j in range(3))
    elif state == 'speaking':
        body += f'<rect x="6" y="7" width="1.5" height="2" rx=".75"/><rect x="10.5" y="7" width="1.5" height="2" rx=".75"/><rect x="7" y="10.5" width="4" height="{1+frame%3*.6}" rx=".5"/>'
    elif state == 'thinking':
        body += '<circle cx="6" cy="9" r=".8"/><circle cx="9" cy="9" r=".8"/><circle cx="12" cy="9" r=".8"/>'
    elif state == 'offline':
        body += '<path d="m3 15 12-12" stroke="#000" stroke-width="1.5"/>'
    elif state == 'error':
        body += '<path d="M9 6v4" stroke="#000" stroke-width="1.5"/><circle cx="9" cy="12" r=".8"/>'
    else:
        body += '<rect x="6" y="7" width="1.5" height="3" rx=".75"/><rect x="10.5" y="7" width="1.5" height="3" rx=".75"/>'
        if state == 'update':
            body += '<circle cx="15" cy="3" r="2.5"/>'
    return body

animation = {'frameDurationMs':160, 'reduceMotion':'use static state icon', 'states':{}}
for state in STATES:
    for pt in [16,18]:
        for scale in [1,2]:
            # fixed 18-unit design box; integral export size, consistent optical center
            source = svg(f'<g fill="#000" transform="scale({pt*scale/18})">{menu(state)}</g>',pt*scale)
            save(f'MenuBar/{state}-{pt}@{scale}x.svg',source)
    if state in ['listening','speaking']:
        animation['states'][state] = []
        for frame in range(4):
            paths=[]
            for pt in [16,18]:
                for scale in [1,2]:
                    name=f'MenuBar/animated/{state}-{frame}-{pt}@{scale}x.svg'
                    save(name, svg(f'<g fill="#000" transform="scale({pt*scale/18})">{menu(state,frame)}</g>',pt*scale))
                    paths.append(name)
            animation['states'][state].append(paths)
save('MenuBar/animations.json',json.dumps(animation,indent=2))

for state in ['idle','listening','thinking','speaking','happy','concerned','sleeping']:
    rendered = face('thinking' if state in ['thinking','sleeping'] else state)
    if state == 'happy':
        rendered += '<path id="expression" d="M420 600q92 90 184 0" fill="none" stroke="#101917" stroke-width="20" stroke-linecap="round"/>'
    elif state == 'concerned':
        rendered += '<path id="expression" d="m350 340 80 30m164 0 80-30" fill="none" stroke="#101917" stroke-width="18" stroke-linecap="round"/>'
    save(f'Face/{state}.svg',svg(rendered))
save('Face/motion.json',json.dumps({'status':'presentation-only; expression protocol is not specified', 'layers':['head','eyes','mouth','expression'], 'channels':{'blink':{'range':[0,1],'durationMs':120,'idleIntervalMs':[3800,6200]},'gazeX':{'range':[-18,18],'unit':'master SVG units'},'gazeY':{'range':[-12,12],'unit':'master SVG units'},'mouthOpen':{'range':[0,1],'height':[16,88],'smoothingMs':70},'listenScale':{'range':[1,1.025],'periodMs':1100},'expressionBlend':{'range':[0,1],'durationMs':180}},'reduceMotion':{'listenScale':1,'blink':0,'expressionBlend':'instant','mouthOpen':'static speaking pose'},'ownership':'surface presentation interpolates; coglet chooses semantic expression after ADR'},indent=2))

for name, body in {
    'welcome': '<rect x="100" y="80" width="440" height="250" rx="20" fill="none" stroke="#64766E" stroke-width="6"/><path d="M260 370h120m-60-40v40" stroke="#64766E" stroke-width="6"/><g transform="translate(185 50) scale(.26)">'+face()+'</g>',
    'permissions':'<rect x="130" y="80" width="380" height="250" rx="25" fill="none" stroke="#64766E" stroke-width="6"/><rect x="284" y="115" width="72" height="130" rx="36" fill="#87EFBC"/><path d="M260 205v10a60 60 0 0 0 120 0v-10m-60 70v35m-35 0h70" fill="none" stroke="#64766E" stroke-width="8"/>',
    'hub':'<rect x="80" y="130" width="160" height="140" rx="20" fill="#87EFBC"/><rect x="400" y="130" width="160" height="140" rx="20" fill="none" stroke="#64766E" stroke-width="6"/><path d="M260 200h120m-20-20 20 20-20 20" fill="none" stroke="#64766E" stroke-width="6"/>'
}.items():
    save(f'Onboarding/{name}.svg',svg(body,640,400))
dmg = svg('<rect width="660" height="440" fill="#F4F7F5"/><text x="50" y="64" font-family="sans-serif" font-size="26" fill="#101917">Softmax for Mac</text><text x="50" y="98" font-family="sans-serif" font-size="15" fill="#43574C">Drag Softmax to Applications.</text><path d="M280 230h100m-18-18 18 18-18 18" fill="none" stroke="#43574C" stroke-width="3"/><text x="50" y="395" font-family="sans-serif" font-size="13" fill="#43574C">Open Softmax from Applications to finish setup.</text>',660,440)
save('DMG/background.svg',dmg)
for scale in [1,2]:
    png(dmg,f'DMG/background@{scale}x.png',660*scale,440*scale)

tokens = json.loads((ROOT/'DesignTokens.json').read_text())
save('DesignTokens.json',json.dumps(tokens,indent=2))
# Swift artifact intentionally generated from the exact JSON; validation compares bytes.
def swift_tokens(t):
    lines=['// Generated by generate-assets.py. Do not edit.', 'import SwiftUI', '', 'enum SoftmaxDesignTokens {']
    for group in ['font','radius','motion']:
        for name,value in t[group].items():
            if isinstance(value,(float,int)):
                lines.append(f'    static let {group}{name[0].upper()+name[1:]}: CGFloat = {value}')
    lines.append('    static let spacing: [CGFloat] = ['+', '.join(map(str,t['space']))+']')
    for theme,colors in t['color'].items():
        lines.append(f'    enum {theme.title()} {{')
        for name,value in colors.items():
            rgb=[int(value[i:i+2],16)/255 for i in [1,3,5]]
            lines.append(f'        static let {name} = Color(.sRGB, red: {rgb[0]:.9f}, green: {rgb[1]:.9f}, blue: {rgb[2]:.9f}, opacity: 1)')
        lines.append('    }')
    return '\n'.join(lines+['}'])+'\n'
save('DesignTokens.swift',swift_tokens(tokens).rstrip())
print('Generated iconset, 60 menu vectors, Face layers, onboarding, DMG, tokens and concept sheet.')
