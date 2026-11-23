import base64
import importlib.util
import os
from pathlib import Path
import plistlib
import sys
import tempfile
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

SCRIPTS=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(SCRIPTS))
from release_policy import PUBLIC_KEY, SPARKLE, configure_bundle, require_mode, validate_appcast, version_for_tag
import release

class ReleasePolicyTests(unittest.TestCase):
    def test_version_grammar(self):
        for tag in ('softmax-mac-v0.0.1-rc1','softmax-mac-v1.2.3','softmax-mac-v1.2.3-beta2'):
            self.assertEqual(version_for_tag(tag),tag.removeprefix('softmax-mac-v'))
        for tag in ('v1.0.0','softmax-mac-v1','softmax-mac-v1.0.0;bad','softmax-mac-v1.0.0/extra'):
            with self.assertRaises(ValueError): version_for_tag(tag)

    def test_adhoc_tag_fail_closed(self):
        require_mode('adhoc','softmax-mac-v0.0.1-rc1','push')
        require_mode('developer-id','softmax-mac-v1.0.0','push')
        with self.assertRaises(ValueError): require_mode('adhoc','softmax-mac-v1.0.0','push')
        with self.assertRaises(ValueError): require_mode('auto','softmax-mac-v1.0.0','push')

    def test_bundle_configuration_and_key_pin(self):
        with tempfile.TemporaryDirectory() as d:
            app=Path(d)/'Softmax.app'; (app/'Contents').mkdir(parents=True)
            info=app/'Contents/Info.plist'
            info.write_bytes(plistlib.dumps({'SUPublicEDKey':PUBLIC_KEY}))
            configure_bundle(app,'1.2.3',17,'developer-id')
            value=plistlib.loads(info.read_bytes())
            self.assertEqual(value['CFBundleVersion'],'17')
            self.assertEqual(value['SoftmaxReleaseChannel'],'developer-id')
            with self.assertRaises(ValueError): configure_bundle(app,'1.2.3',17,'developer-id','proof-key')
            configure_bundle(app,'1.2.3',17,'adhoc','proof-key')
            self.assertEqual(plistlib.loads(info.read_bytes())['SUPublicEDKey'],'proof-key')
            info.write_bytes(plistlib.dumps({'SUPublicEDKey':'wrong'}))
            with self.assertRaises(ValueError): configure_bundle(app,'1.2.3',17,'developer-id')

    def test_appcast_rejects_wrong_versions_url_length_and_signature(self):
        with tempfile.TemporaryDirectory() as d:
            dmg=Path(d)/'Softmax-1.0.0.dmg'; dmg.write_bytes(b'test archive')
            xml=Path(d)/'appcast.xml'; prefix='https://example.test/'
            root=ET.Element('rss'); channel=ET.SubElement(root,'channel'); item=ET.SubElement(channel,'item')
            ET.SubElement(item,SPARKLE+'version').text='4'
            ET.SubElement(item,SPARKLE+'shortVersionString').text='1.0.0'
            enc=ET.SubElement(item,'enclosure',{'url':prefix+dmg.name,'length':str(dmg.stat().st_size),SPARKLE+'edSignature':base64.b64encode(bytes(64)).decode()})
            def save(): ET.ElementTree(root).write(xml)
            save(); self.assertEqual(validate_appcast(xml,dmg,'1.0.0',4,prefix),enc.get(SPARKLE+'edSignature'))
            for name,value in [('url','http://wrong.test/'),('length','0'),(SPARKLE+'edSignature','AAAA')]:
                old=enc.get(name); enc.set(name,value); save()
                with self.assertRaises(ValueError): validate_appcast(xml,dmg,'1.0.0',4,prefix)
                enc.set(name,old)
            save()
            with self.assertRaises(ValueError): validate_appcast(xml,dmg,'1.0.0',5,prefix)
            with self.assertRaises(ValueError): validate_appcast(xml,dmg,'2.0.0',4,prefix)
            ET.SubElement(channel,'item'); save()
            with self.assertRaises(ValueError): validate_appcast(xml,dmg,'1.0.0',4,prefix)

    def test_inside_out_signing_runtime_entitlements_only_outer_app(self):
        with tempfile.TemporaryDirectory() as d:
            app=Path(d)/'Softmax.app'; framework=app/'Contents/Frameworks/Test.framework'
            framework.mkdir(parents=True)
            binary=framework/'Test'; binary.write_bytes(b'\xcf\xfa\xed\xfe\0')
            calls=[]
            with patch.object(release,'run',side_effect=lambda *a,**k:calls.append(tuple(map(str,a)))):
                release.sign_tree(app,'identity','developer-id')
            signatures=[c for c in calls if '--sign' in c]
            self.assertEqual([c[-1] for c in signatures],[str(binary),str(framework),str(app)])
            self.assertTrue(all('--timestamp' in c and 'runtime' in c for c in signatures))
            self.assertTrue(all('--deep' not in c for c in signatures))
            self.assertEqual(sum('--entitlements' in c for c in signatures),1)
            self.assertIn('--strict',calls[-1])

    def test_minimal_entitlements(self):
        ent=plistlib.loads((SCRIPTS.parent/'release.entitlements').read_bytes())
        self.assertEqual(set(ent),{'com.apple.security.device.audio-input','com.apple.security.network.client','com.apple.security.network.server'})
        self.assertTrue(all(ent.values()))

    def test_sparkle_seed_format_and_production_key_pin(self):
        from types import SimpleNamespace
        with patch.dict(os.environ,{'OPENSSL':'openssl','SPARKLE_PRIVATE_KEY':base64.b64encode(bytes(32)).decode()}):
            result=SimpleNamespace(stdout=b'public-der-prefix'+base64.b64decode(PUBLIC_KEY))
            with patch.object(release.subprocess,'run',return_value=result):
                private,public=release.prepare_sparkle_key('developer-id')
                self.assertEqual(len(base64.b64decode(private)),32)
                self.assertEqual(public,PUBLIC_KEY)
            with patch.object(release.subprocess,'run',return_value=SimpleNamespace(stdout=bytes(44))):
                with self.assertRaises(ValueError): release.prepare_sparkle_key('developer-id')
        with patch.dict(os.environ,{'SPARKLE_PRIVATE_KEY':base64.b64encode(bytes(64)).decode()}):
            with self.assertRaises(ValueError): release.prepare_sparkle_key('adhoc')

    def test_notary_rejection_is_fatal(self):
        with patch.object(release,'run',side_effect=['{"id":"test-rejection","status":"Invalid"}',None]):
            with self.assertRaises(RuntimeError): release.notarize(Path('app.zip'),Path('temp'))

if __name__=='__main__': unittest.main()
