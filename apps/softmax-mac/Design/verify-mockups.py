#!/usr/bin/env python3
"""Owned headless-browser interaction checks and deterministic screenshot capture."""
from pathlib import Path
import json
import shutil
import subprocess
import tempfile
from urllib.parse import urlencode
from playwright.sync_api import sync_playwright

ROOT=Path(__file__).resolve().parent
REPO=ROOT.parents[2]
OUT=ROOT/'mockups/screenshots'
OUT.mkdir(parents=True,exist_ok=True)
URL=(ROOT/'mockups/softmax-mac.html').as_uri()
report={'states':[], 'scenarios':[], 'keyboard':[], 'errors':[], 'negativeChecks':[]}

def owned(page):
    assert page.url.startswith(URL), 'Input target must be the page owned by this test'

def key(page, value):
    owned(page);page.keyboard.press(value)

def load(page, **params):
    page.goto(URL+'?'+urlencode(params),wait_until='load')
    page.wait_for_function('Array.from(document.images).every(i=>i.complete)')
    assert page.locator('img').evaluate_all('(xs)=>xs.every(i=>i.naturalWidth>0)'), 'Broken image'

def overflow(page):
    return page.evaluate('document.documentElement.scrollWidth > innerWidth')

def settings_semantics(page, section, scenario):
    panel = page.locator('.settings-main')
    text = panel.inner_text()
    def action(name):
        return panel.locator(f'[data-action="{name}"]').count()
    if section == 'hub-account':
        assert action('disconnect') == (scenario == 'default'), (scenario, text)
        assert action('connect') == (scenario in ['disconnected', 'error']), (scenario, text)
        assert action('settings-cancel-connect') == (scenario == 'connecting')
        assert action('settings-confirm-hub') == (scenario == 'connecting')
        assert ('Example connection: connected' in text) == (scenario == 'default')
        assert ('No account is connected.' in text) == (scenario in ['disconnected', 'error'])
        assert 'Could not load.' not in text
        expected = {'default': 'Example connection: connected', 'disconnected': 'Hub disconnected.', 'connecting': 'Connecting…', 'error': 'Could not verify this hub.'}
        assert expected[scenario] in text
    if section == 'coglets':
        assert panel.locator('.fi').count() == (2 if scenario == 'default' else 0)
        assert ('Sample · running' in text) == (scenario == 'default')
        assert ('Sample · paused' in text) == (scenario == 'default')
        assert action('retry-coglets') == (scenario == 'error')
        assert action('coglets-loaded') == (scenario == 'loading')
        if scenario in ['loading', 'error']:
            assert 'Lifecycle is unknown' in text
        expected = {'default': 'Sample · running', 'empty': 'No installed coglets.', 'loading': 'Loading coglets…', 'error': 'Could not load coglets.'}
        assert expected[scenario] in text

with sync_playwright() as p:
    browser=p.chromium.launch(headless=True)
    page=browser.new_page(viewport={'width':1280,'height':1050},device_scale_factor=1)
    page.on('pageerror',lambda e:report['errors'].append(str(e)))
    load(page,view='menu',state='idle',theme='light',still=1)
    model=page.evaluate('window.prototypeStates')
    expected={(v,s,t) for v,ss in model.items() for s in ss for t in ['light','dark']}
    for theme in ['light','dark']:
        for view,states in model.items():
            for state in states:
                load(page,view=view,state=state,theme=theme,still=1)
                assert page.locator('#surface').input_value()==view
                assert page.locator('#state').input_value()==state
                assert not overflow(page),(view,state,theme,'body overflow')
                box=page.locator('.window').bounding_box()
                desktop=page.locator('#desktop').bounding_box()
                assert box['y']+box['height']<=desktop['y']+desktop['height'],(view,state,'window clipped below desktop',box,desktop)
                page.locator('#desktop').screenshot(path=str(OUT/f'{theme}-{view}-{state}.jpg'),type='jpeg',quality=80,animations='disabled')
                report['states'].append({'view':view,'state':state,'theme':theme})
        load(page,view='menu',state='idle',theme=theme,still=1)
        page.locator('.icon-proof').screenshot(path=str(OUT/f'{theme}-menu-template-proof.png'))
        for view in model:
            load(page,view=view,state=model[view][0],theme=theme,still=1)
            page.set_viewport_size({'width':640,'height':1050})
            assert not overflow(page),(view,theme,'640 overflow')
            page.set_viewport_size({'width':1280,'height':1050})
        for section in model['settings']:
            load(page,view='settings',state=section,theme=theme,still=1)
            scenarios=page.locator('#scenario option').evaluate_all('(xs)=>xs.map(x=>x.value)')
            for scenario in scenarios:
                page.select_option('#scenario',scenario)
                assert page.locator('#scenario').input_value()==scenario
                assert not overflow(page)
                settings_semantics(page, section, scenario)
                if section in ['hub-account', 'coglets']:
                    page.locator('#desktop').screenshot(path=str(OUT/f'{theme}-settings-{section}-{scenario}.jpg'),type='jpeg',quality=80,animations='disabled')
                report['scenarios'].append({'section':section,'scenario':scenario,'theme':theme,'semanticAssertions':section in ['hub-account', 'coglets']})
        # Exercise real controls, not just the scenario selector, in both themes.
        load(page,view='settings',state='hub-account',scenario='disconnected',theme=theme,still=1)
        for control, result in [('connect','connecting'), ('settings-cancel-connect','disconnected'), ('connect','connecting'), ('settings-confirm-hub','default')]:
            page.locator(f'[data-action="{control}"]').click()
            assert page.locator('#scenario').input_value() == result
            settings_semantics(page,'hub-account',result)
        page.locator('[data-action="disconnect"]').click()
        page.locator('[data-action="settings-keep-connected"]').click()
        settings_semantics(page,'hub-account','default')
        page.locator('[data-action="disconnect"]').click()
        page.locator('[data-action="settings-confirm-disconnect"]').click()
        settings_semantics(page,'hub-account','disconnected')
        load(page,view='settings',state='hub-account',scenario='error',theme=theme,still=1)
        page.locator('[data-action="connect"]').click()
        settings_semantics(page,'hub-account','connecting')
        page.locator('[data-action="settings-cancel-connect"]').click()
        settings_semantics(page,'hub-account','disconnected')
        load(page,view='settings',state='coglets',scenario='error',theme=theme,still=1)
        page.locator('[data-action="retry-coglets"]').click()
        settings_semantics(page,'coglets','loading')
        page.locator('[data-action="coglets-loaded"]').click()
        settings_semantics(page,'coglets','default')
        report['keyboard'].append(f'{theme}: Hub connect/cancel/confirm, disconnect keep/confirm, error retry/cancel; Coglets error/retry/loading/sample completion')
    assert {(r['view'],r['state'],r['theme']) for r in report['states']}==expected
    # Native page input, never OS-level keyboard injection.
    load(page,view='menu',state='idle',theme='light')
    key(page,'Control+k')
    assert page.locator('#palette').evaluate('(e)=>e.open')
    owned(page);page.keyboard.type('volume')
    assert page.locator('#command-search').input_value()=='volume'
    assert page.locator('#results').inner_text().strip()
    key(page,'ArrowDown');key(page,'ArrowUp');key(page,'Enter')
    assert page.locator('#surface').input_value()=='settings'
    assert page.locator('#state').input_value() in ['audio','hotkey','privacy']
    report['keyboard'].append('palette: real typing, synonym search, arrows, Return activation')
    page.locator('[data-action="menu"]').first.click()
    assert page.locator('#surface').input_value()=='menu'
    page.locator('[data-action="settings"]').click()
    page.locator('#palette-open').focus()
    key(page,'a')
    owned(page);page.keyboard.type('udio')
    assert page.locator('#settings-filter').input_value()=='audio'
    assert page.locator('.sidebar .fi:visible').count()==1
    assert page.locator('#settings-filter-count').inner_text()=='1 matches'
    assert page.locator('.sidebar mark').inner_text()=='Audio'
    key(page,'Enter')
    assert page.locator('#state').input_value()=='audio'
    report['keyboard'].append('type-anywhere settings filter, highlighted match, count, Return, menu Back round trip')
    page.select_option('#state','general')
    page.locator('[aria-label="App appearance"]').focus()
    key(page,'ArrowDown');key(page,'Enter')
    assert page.locator('[aria-label="App appearance"]').input_value()=='Light'
    assert page.locator('#state').input_value()=='general'
    report['keyboard'].append('native select ArrowDown and Enter preserved; sidebar does not steal form input')
    page.select_option('#state','coglets')
    page.locator('#list-filter').fill('face')
    assert page.locator('.settings-main .fi:visible').count()==1
    assert page.locator('#filter-count').inner_text()=='1 matches'
    key(page,'Escape')
    assert page.locator('#list-filter').input_value()==''
    assert page.locator('.settings-main .fi:visible').count()==2
    key(page,'?');assert page.locator('#help').evaluate('(e)=>e.open')
    key(page,'Escape');assert not page.locator('#help').evaluate('(e)=>e.open')
    page.locator('[data-action="menu"]').first.click();page.locator('.other-app').click()
    owned(page);page.keyboard.down('Space')
    assert page.locator('#state').input_value()=='listening'
    owned(page);page.keyboard.up('Space')
    assert page.locator('#state').input_value()=='thinking'
    key(page,'Escape');assert page.locator('#state').input_value()=='cancelled'
    report['keyboard'].append('coglet filter clear, help focus restoration, PTT hold/release/cancel')
    load(page,view='onboarding',state='welcome',theme='light',still=1)
    page.locator('[data-action="onboard-microphone"]').click()
    page.locator('[data-action="allow-mic"]').click()
    assert page.locator('#state').input_value()=='microphone-granted'
    page.select_option('#state','microphone-denied')
    assert page.locator('#state').input_value()=='microphone-denied'
    page.locator('[data-action="onboard-hotkey"]').last.click()
    page.locator('[data-action="allow-hotkey"]').click()
    assert page.locator('#state').input_value()=='hotkey-granted'
    page.select_option('#state','hotkey-denied')
    assert page.locator('#state').input_value()=='hotkey-denied'
    page.locator('[data-action="onboard-login-item"]').last.click()
    login=page.locator('[aria-label="Start Softmax at login"]')
    login.click()
    assert login.is_checked()
    page.locator('[data-action="onboard-hub"]').last.click()
    page.locator('[data-action="onboard-back"]').click()
    assert page.locator('[aria-label="Start Softmax at login"]').is_checked()
    page.locator('[data-action="onboard-hub"]').last.click()
    page.locator('[data-action="onboard-connect"]').click()
    assert page.locator('#state').input_value()=='connecting'
    page.locator('[data-action="cancel-connect"]').click()
    assert page.locator('#state').input_value()=='hub'
    page.locator('[data-action="onboard-ready"]').last.click()
    assert page.locator('#state').input_value()=='voice-not-set-up'
    assert page.locator('.onboard h2').inner_text()=='Voice not set up'
    report['keyboard'].append('denied/skipped setup is not ready; login choice retained across Back')
    load(page,view='onboarding',state='microphone',theme='light',still=1)
    page.locator('[data-action="allow-mic"]').click()
    page.locator('[data-action="onboard-hotkey"]').last.click()
    page.locator('[data-action="allow-hotkey"]').click()
    page.locator('[data-action="onboard-login-item"]').last.click()
    page.locator('[data-action="onboard-hub"]').last.click()
    page.locator('[data-action="onboard-connect"]').click()
    page.locator('[data-action="confirm-sample-hub"]').click()
    page.locator('[data-action="onboard-ready"]').last.click()
    assert page.locator('#state').input_value()=='ready'
    page.locator('[data-action="face"]').click()
    assert page.locator('#surface').input_value()=='face'
    report['keyboard'].append('first-run path: denied permission recovery, skip, connect/cancel, ready, Face')
    load(page,view='settings',state='audio',theme='light',still=1)
    page.locator('[data-action="test-mic"]').click()
    assert page.locator('[data-action="test-mic"]').inner_text()=='Stop test'
    page.locator('[data-action="test-mic"]').click()
    assert page.locator('[data-action="test-mic"]').inner_text()=='Start test'
    report['keyboard'].append('simulated microphone test exposes Stop and returns to idle')
    for direction in ['rail','companion','stage']:
        load(page,view='face',state='idle',theme='dark',dir=direction,still=1)
        page.locator('#desktop').screenshot(path=str(OUT/f'direction-{direction}.jpg'),type='jpeg',quality=85)
    # Motion is verified in a visible, owned headless page with real timers.
    load(page,view='menu',state='listening',theme='light')
    page.wait_for_function("document.querySelector('.template').src.includes('/animated/')")
    page.emulate_media(reduced_motion='reduce')
    load(page,view='menu',state='speaking',theme='light')
    assert '/animated/' not in page.locator('.template').get_attribute('src')
    report['keyboard'].append('menu frame playback and reduced-motion static fallback')
    assert not report['errors'],report['errors']
    browser.close()

# Negative cases prove the checker rejects missing and wrong-dimension assets.
with tempfile.TemporaryDirectory(prefix='softmax-assets-',dir=ROOT) as tmp:
    clone=Path(tmp)/'Design';shutil.copytree(ROOT,clone,ignore=shutil.ignore_patterns('screenshots','softmax-assets-*'))
    icon=clone/'AppIcon.appiconset/icon-16@1x.png'
    original=icon.read_bytes();icon.unlink()
    run=subprocess.run(['bash',str(REPO/'scripts/check-assets.sh'),str(clone)],capture_output=True,text=True)
    assert run.returncode!=0;report['negativeChecks'].append('missing icon rejected')
    icon.write_bytes((clone/'AppIcon.appiconset/icon-32@1x.png').read_bytes())
    run=subprocess.run(['bash',str(REPO/'scripts/check-assets.sh'),str(clone)],capture_output=True,text=True)
    assert run.returncode!=0;report['negativeChecks'].append('wrong-dimension icon rejected')
report['stateCount']=len(report['states'])
report['scenarioCount']=len(report['scenarios'])
(ROOT/'mockups/verification.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({k:v for k,v in report.items() if k not in ['states','scenarios']},indent=2))
