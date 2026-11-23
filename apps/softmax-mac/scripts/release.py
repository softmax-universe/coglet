#!/usr/bin/env python3
"""Archive, inside-out sign, notarize, staple, package and validate. No fallback."""
import argparse
import base64
import json
import os
from pathlib import Path
import plistlib
import secrets
import subprocess
import sys
from release_policy import IDENTITY, TEAM, PUBLIC_KEY, configure_bundle, require_mode, version_for_tag, validate_appcast

ROOT = Path(__file__).resolve().parents[1]

def run(*args, input=None, capture=False, sensitive=False):
    # Never print commands containing credentials, including on subprocess failure.
    if not sensitive:
        print('+', ' '.join(map(str,args)), flush=True)
    p = subprocess.run(list(map(str,args)), input=input, text=True, capture_output=capture or sensitive)
    if p.returncode:
        if capture and not sensitive:
            print(p.stdout, p.stderr, file=sys.stderr)
        raise RuntimeError('Release command failed: ' + str(args[0]))
    return p.stdout if capture else ''


def sign_tree(app, identity, mode):
    options = ['--options', 'runtime', '--timestamp'] if mode == 'developer-id' else ['--timestamp=none']
    # Resolve framework-version symlinks and sign every real Mach-O before its
    # containing .xpc/.app/.framework, deepest first. Never --deep for signing.
    candidates = set()
    for p in app.rglob('*'):
        if p.is_symlink():
            continue
        if p.is_dir() and p.suffix in ('.framework','.app','.xpc'):
            candidates.add(p)
        elif p.is_file():
            with p.open('rb') as f:
                magic = f.read(4)
            if magic in (b'\xcf\xfa\xed\xfe', b'\xce\xfa\xed\xfe', b'\xfe\xed\xfa\xcf', b'\xfe\xed\xfa\xce', b'\xca\xfe\xba\xbe', b'\xbe\xba\xfe\xca', b'\xca\xfe\xba\xbf'):
                candidates.add(p)
    for p in sorted(candidates, key=lambda x: (-len(x.parts),str(x))):
        run('codesign','--force','--sign',identity,*options,p)
    run('codesign','--force','--sign',identity,*options,'--entitlements',ROOT/'release.entitlements',app)
    run('codesign','--verify','--deep','--strict','--verbose=2',app)


def notarize(path, temp):
    # Password is held by a temporary keychain profile; API keys can replace this
    # profile creation without changing the packaging or verification gates.
    output = run('xcrun','notarytool','submit',path,'--keychain',temp/'release.keychain-db',
                 '--keychain-profile','softmax-notary','--wait','--timeout','30m','--output-format','json',capture=True)
    result = json.loads(output)
    print('notarytool:', result['id'], result['status'], flush=True)
    if result['status'] != 'Accepted':
        run('xcrun','notarytool','log',result['id'],'--keychain',temp/'release.keychain-db',
            '--keychain-profile','softmax-notary')
        raise RuntimeError('Apple rejected notarization')


def prepare_sparkle_key(mode):
    private = os.environ.get('SPARKLE_PRIVATE_KEY')
    if mode == 'developer-id' and not private:
        raise ValueError('Developer ID releases require the production Sparkle key')
    seed = base64.b64decode(private, validate=True) if private else secrets.token_bytes(32)
    if len(seed) != 32:
        raise ValueError('Sparkle requires a base64 32-byte Ed25519 seed')
    der=b'\x30\x2e\x02\x01\x00\x30\x05\x06\x03\x2b\x65\x70\x04\x22\x04\x20'+seed
    result=subprocess.run([os.environ['OPENSSL'],'pkey','-inform','DER','-pubout','-outform','DER'],input=der,capture_output=True,check=True)
    public=base64.b64encode(result.stdout[-32:]).decode()
    if mode == 'developer-id' and public != PUBLIC_KEY:
        raise ValueError('Production Sparkle private/public key mismatch')
    return base64.b64encode(seed).decode(), public


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--mode',choices=['developer-id','adhoc'],required=True)
    parser.add_argument('--tag',required=True)
    args=parser.parse_args()
    require_mode(args.mode,args.tag,os.environ.get('GITHUB_EVENT_NAME','workflow_dispatch'))
    version=version_for_tag(args.tag)
    build=os.environ['GITHUB_RUN_NUMBER']
    temp=Path(os.environ['RUNNER_TEMP'])/'softmax-release'
    temp.mkdir(mode=0o700)
    out=Path(os.environ['RUNNER_TEMP'])/'softmax-output'
    out.mkdir()
    keychain=temp/'release.keychain-db'
    old_keychains=[]
    try:
        identity='-'
        if args.mode=='developer-id':
            assert os.environ['TEAM_ID']==TEAM, 'Unexpected Developer ID team'
            if not os.environ.get('SPARKLE_PRIVATE_KEY'):
                raise ValueError('Developer ID releases require the production Sparkle key')
            old_keychains=run('security','list-keychains','-d','user',capture=True).replace('"','').split()
            keypass=secrets.token_urlsafe(32)
            run('security','create-keychain','-p',keypass,keychain,sensitive=True)
            run('security','set-keychain-settings','-lut','21600',keychain)
            run('security','unlock-keychain','-p',keypass,keychain,sensitive=True)
            cert=temp/'certificate.p12'
            cert.write_bytes(base64.b64decode(os.environ['CERTIFICATE_P12'],validate=True))
            cert.chmod(0o600)
            run('security','import',cert,'-k',keychain,'-P',os.environ['CERTIFICATE_PASSWORD'],
                '-T','/usr/bin/codesign',sensitive=True)
            run('security','set-key-partition-list','-S','apple-tool:,apple:,codesign:','-s','-k',keypass,keychain,sensitive=True)
            run('security','list-keychains','-d','user','-s',keychain,*old_keychains)
            identities=run('security','find-identity','-v','-p','codesigning',keychain,capture=True)
            if IDENTITY not in identities or 'Developer ID Application:' not in identities:
                raise RuntimeError('Imported identity is not the pinned Developer ID')
            identity=IDENTITY
            run('xcrun','notarytool','store-credentials','softmax-notary','--keychain',keychain,
                '--apple-id',os.environ['APPLE_ID'],'--team-id',TEAM,'--password',os.environ['NOTARY_PASSWORD'],sensitive=True)
        run('xcodegen','generate','--spec',ROOT/'project.yml')
        archive=temp/'Softmax.xcarchive'
        run('xcodebuild','archive','-project',ROOT/'Softmax.xcodeproj','-scheme','Softmax',
            '-configuration','Release','-destination','generic/platform=macOS',
            '-archivePath',archive,'CODE_SIGNING_ALLOWED=NO','SKIP_INSTALL=NO',
            'ARCHS=arm64 x86_64','ONLY_ACTIVE_ARCH=NO')
        app=archive/'Products/Applications/Softmax.app'
        private, public = prepare_sparkle_key(args.mode)
        configure_bundle(app,version,build,args.mode,public if args.mode=='adhoc' else None)
        sign_tree(app,identity,args.mode)
        if args.mode=='developer-id':
            details=run('codesign','-d','--verbose=4',app,capture=True)
            # codesign details are on stderr; verification and policy assessment
            # below are authoritative, and log their evidence directly.
            zipped=temp/'Softmax-notary.zip'
            run('ditto','-c','-k','--keepParent',app,zipped)
            notarize(zipped,temp)
            run('xcrun','stapler','staple',app)
            run('xcrun','stapler','validate',app)
            run('spctl','--assess','--type','execute','--verbose=4',app)
        # A staging copy prevents create-dmg from modifying the signed archive.
        stage=temp/'dmg-source'
        stage.mkdir()
        run('ditto',app,stage/'Softmax.app')
        dmg=out/f'Softmax-{version}.dmg'
        background=ROOT/'Design/DMG/background@1x.png'
        flags=['--background',background] if background.is_file() else []
        run('create-dmg','--volname','Softmax','--window-size','660','440',
            '--icon-size','128','--icon','Softmax.app','180','230',
            '--app-drop-link','480','230',*flags,dmg,stage)
        if args.mode=='developer-id':
            run('codesign','--force','--sign',identity,'--timestamp',dmg)
            notarize(dmg,temp)
            run('xcrun','stapler','staple',dmg)
            run('xcrun','stapler','validate',dmg)
            # Apple's DMG policy is 'open/primary-signature'; 'install' is for PKGs.
            run('spctl','-a','-vvv','-t','open','--context','context:primary-signature',dmg)
        else:
            run('codesign','--force','--sign','-','--timestamp=none',dmg)
            print('AD-HOC PROOF ONLY: not notarized; Gatekeeper acceptance is NOT claimed',flush=True)
        run('codesign','--verify','--strict','--verbose=2',dmg)
        # Signed feed generated by the actual Sparkle tools, not hand-written XML.
        tools=Path(os.environ['SPARKLE_BIN'])
        prefix=f'https://github.com/softmax-universe/coglet/releases/download/{args.tag}/'
        run(tools/'sign_update','--ed-key-file','-','-p',dmg,input=private,capture=True)
        run(tools/'generate_appcast','--ed-key-file','-','--maximum-versions','1','--maximum-deltas','0',
            '--download-url-prefix',prefix,out,input=private)
        signature=validate_appcast(out/'appcast.xml',dmg,version,build,prefix)
        run(tools/'sign_update','--ed-key-file','-','--verify',dmg,signature,input=private)
        (out/'verification.json').write_text(json.dumps({'tag':args.tag,'version':version,'build':build,
            'mode':args.mode,'source':os.environ.get('GITHUB_SHA'),'notarized':args.mode=='developer-id',
            'sparkle_signature_verified':True,'production_key':bool(os.environ.get('SPARKLE_PRIVATE_KEY'))},indent=2))
        print('VERIFIED:',out,flush=True)
    finally:
        if old_keychains:
            run('security','list-keychains','-d','user','-s',*old_keychains)
        if keychain.exists():
            run('security','delete-keychain',keychain)
        (temp/'certificate.p12').unlink(missing_ok=True)

if __name__=='__main__':
    try:
        main()
    except Exception as exc:
        # subprocess exceptions can carry argv with secrets; report type only.
        print('Release failed:',type(exc).__name__,str(exc) if isinstance(exc,(ValueError,RuntimeError,AssertionError)) else '',file=sys.stderr)
        sys.exit(1)
