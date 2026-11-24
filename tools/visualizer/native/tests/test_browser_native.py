"""Named native M2 browser acceptance; never substitutes display-only fixtures."""
import json
import os
from pathlib import Path
import threading

from playwright.sync_api import sync_playwright, expect
from test_http import native_source, load_server


def test_native_m2_load_scrub_replay_same_hash_and_receipt_what_if_divergence(native_source, tmp_path):
    archive, helper, trace = native_source
    server = load_server().make_server(tmp_path / 'unused', native_trace=archive, native_helper=helper)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with sync_playwright() as pw:
            browser = pw.chromium.launch()
            page = browser.new_page(viewport={'width': 1440, 'height': 1000})
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.goto(server.url)
            page.get_by_role('button', name='Load native archive', exact=True).click(timeout=2000)
            expect(page.get_by_test_id('native-status')).to_contain_text('All native state hashes match', timeout=30000)
            expect(page.get_by_test_id('native-final')).to_have_text(trace['final_hash'])
            expect(page.get_by_test_id('hash')).to_have_text(trace['final_hash'])
            latest = bytes.fromhex(trace['records'][-1]['output_files']['/data/inbox.jsonl']).decode()
            expect(page.get_by_test_id('files')).to_contain_text(latest)
            slider = page.get_by_role('slider', name='Native step', exact=True)
            slider.focus()
            page.keyboard.press('Home')
            for _ in range(2):
                page.keyboard.press('ArrowRight')
            record = trace['records'][2]
            earlier = bytes.fromhex(record['output_files']['/data/inbox.jsonl']).decode()
            assert earlier and earlier != latest
            expect(page.get_by_test_id('files')).to_contain_text(earlier)
            expect(page.get_by_test_id('hash')).to_have_text(record['output_hash'])
            page.get_by_label('Receipt index').fill('0')
            page.get_by_label('Edited receipt result JSON').fill('{"what_if":true}')
            page.get_by_role('button', name='Fork native receipt', exact=True).click()
            expect(page.get_by_test_id('native-status')).to_contain_text('First native divergence', timeout=30000)
            expect(page.get_by_test_id('native-final')).not_to_have_text(trace['final_hash'])
            expect(page.get_by_test_id('native-baseline')).to_have_text(trace['final_hash'])
            actual = page.get_by_test_id('files').locator('pre').all_text_contents()
            inboxes = [text for text in actual if 'what_if' in text]
            assert len(inboxes) == 1
            messages = [json.loads(line) for line in inboxes[0].splitlines()]
            assert len(messages) == 4
            assert messages[0]['event']['payload']['result'] == {'what_if': True}
            assert all(m['event']['payload']['result'] == {} for m in messages[1:])
            assert '/code/' not in page.get_by_test_id('files').inner_text()
            page.get_by_label('Edited receipt result JSON').fill('{')
            page.get_by_role('button', name='Fork native receipt', exact=True).click()
            expect(page.get_by_test_id('native-status')).to_contain_text('unavailable')
            expect(page.get_by_test_id('files')).to_be_empty()
            expect(page.get_by_test_id('native-final')).to_be_empty()
            page.get_by_role('button', name='Replay native baseline', exact=True).click()
            expect(page.get_by_test_id('native-final')).to_have_text(trace['final_hash'], timeout=30000)
            expect(page.get_by_test_id('files')).to_contain_text(latest)
            proof = Path(os.environ.get('VISUALIZER_PROOF_DIR', tmp_path))
            proof.mkdir(parents=True, exist_ok=True)
            for width in (1440, 390):
                page.set_viewport_size({'width': width, 'height': 1000})
                assert page.evaluate('document.documentElement.scrollWidth <= innerWidth')
                page.screenshot(path=str(proof / f'native-replay-{width}.png'), full_page=True)
            delayed = []
            page.route('**/api/native', lambda route: delayed.append(route))
            page.get_by_role('button', name='Replay native baseline', exact=True).click()
            expect(page.get_by_test_id('native-status')).to_contain_text('Re-executing')
            expect(page.get_by_test_id('files')).to_be_empty()
            assert len(delayed) == 1
            page.get_by_role('button', name='Follow live', exact=True).click()
            response = delayed[0].fetch()
            delayed[0].fulfill(response=response)
            expect(page.get_by_test_id('native-final')).to_be_empty()
            expect(page.get_by_test_id('hash')).to_be_empty()
            assert errors == []
            browser.close()
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
