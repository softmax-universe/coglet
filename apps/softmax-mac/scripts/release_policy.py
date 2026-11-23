"""Softmax release policy and appcast checks; stdlib, also tested on Linux."""
import base64
import plistlib
import re
import xml.etree.ElementTree as ET
from pathlib import Path

SPARKLE = '{http://www.andymatuschak.org/xml-namespaces/sparkle}'
PUBLIC_KEY = 'EHjimh35CKewwWRhSL0BiMhe6Lx0yqlQ0+j4jDcRSs0='
IDENTITY = '333D6D009F5B83C7CBB1C21439F7A34F3C070ECD'
TEAM = 'Z7589R323Y'

def version_for_tag(tag):
    match = re.fullmatch(r'softmax-mac-v(\d+\.\d+\.\d+(?:-(?:rc|beta)\d+)?)', tag)
    if not match:
        raise ValueError('Expected softmax-mac-vX.Y.Z[-rcN|-betaN]')
    return match[1]

def require_mode(mode, tag, event):
    version_for_tag(tag)
    if mode not in ('developer-id', 'adhoc'):
        raise ValueError('Unknown signing mode')
    if mode == 'adhoc' and event == 'push' and tag != 'softmax-mac-v0.0.1-rc1':
        raise ValueError('Ad-hoc tag publication allowed only for the named proof tag')

def validate_appcast(path, dmg, version, build, prefix):
    root = ET.parse(path).getroot()
    items = root.findall('./channel/item')
    if len(items) != 1:
        raise ValueError('Expected exactly one full update')
    item = items[0]
    enclosure = item.find('enclosure')
    if enclosure is None:
        raise ValueError('Missing enclosure')
    def field(name):
        return item.findtext(SPARKLE + name) or enclosure.get(SPARKLE + name)
    if field('version') != str(build) or field('shortVersionString') != version:
        raise ValueError('Appcast bundle version mismatch')
    if enclosure.get('url') != prefix + Path(dmg).name:
        raise ValueError('Appcast URL mismatch')
    if enclosure.get('length') != str(Path(dmg).stat().st_size):
        raise ValueError('Appcast length mismatch')
    signature = enclosure.get(SPARKLE + 'edSignature', '')
    if len(base64.b64decode(signature, validate=True)) != 64:
        raise ValueError('Missing/invalid Ed25519 signature')
    return signature

def configure_bundle(app, version, build, mode, proof_public_key=None):
    path = Path(app) / 'Contents/Info.plist'
    data = plistlib.loads(path.read_bytes())
    if data.get('SUPublicEDKey') != PUBLIC_KEY:
        raise ValueError('Bundle Sparkle public key mismatch')
    if proof_public_key is not None:
        if mode != 'adhoc':
            raise ValueError('Proof key forbidden in production bundle')
        data['SUPublicEDKey'] = proof_public_key
    data.update(CFBundleShortVersionString=version, CFBundleVersion=str(build), SoftmaxReleaseChannel=mode)
    path.write_bytes(plistlib.dumps(data))
