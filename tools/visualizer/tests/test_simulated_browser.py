import json
import os
from pathlib import Path
import threading

from playwright.sync_api import sync_playwright, expect
from test_stream import load
from test_simulation import archive, load_sim


def test_recorded_m2_upload_scrub_step_replay_same_final_hash_and_edited_input_divergence(tmp_path):
    source = archive()
    expected = load_sim().replay(source)
    path = tmp_path / 'm2.json'
    path.write_text(json.dumps(source))
    server = load().make_server(tmp_path / 'unused')
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with sync_playwright() as pw:
            browser = pw.chromium.launch()
            page = browser.new_page(viewport={'width': 1440, 'height': 1000})
            errors = []
            page.on('pageerror', lambda e: errors.append(str(e)))
            page.goto(server.url)
            page.get_by_label('Load simulation archive').set_input_files(path)
            expect(page.get_by_test_id('simulation-status')).to_contain_text('All world hashes match')
            expect(page.get_by_test_id('simulation-hash')).to_have_text(expected['frames'][-1]['hash'])
            page.get_by_role('slider', name='World step', exact=True).focus()
            page.keyboard.press('Home')
            page.keyboard.press('ArrowRight')
            expect(page.get_by_test_id('simulation-step')).to_have_text('World step 1 / 4')
            expect(page.get_by_test_id('simulation-hash')).to_have_text(expected['frames'][1]['hash'])
            page.get_by_role('button', name='Next world step', exact=True).click()
            expect(page.get_by_test_id('simulation-step')).to_have_text('World step 2 / 4')
            page.get_by_role('button', name='Replay recorded inputs', exact=True).click()
            expect(page.get_by_test_id('simulation-hash')).to_have_text(expected['frames'][-1]['hash'])
            page.get_by_role('slider', name='World step', exact=True).focus()
            page.keyboard.press('Home')
            page.keyboard.press('ArrowRight')
            page.keyboard.press('ArrowRight')
            page.get_by_label('Action at selected world step').select_option('north')
            page.get_by_role('button', name='Run what-if', exact=True).click()
            expect(page.get_by_test_id('simulation-status')).to_contain_text('First divergence: world step 3')
            expect(page.get_by_test_id('simulation-hash')).not_to_have_text(expected['frames'][-1]['hash'])
            expect(page.get_by_test_id('simulation-scope')).to_contain_text('Guest hashes are recorded, not verified')
            # Broken uploads clear the previous result; then recovery works.
            page.get_by_label('Load simulation archive').set_input_files({'name': 'bad.json', 'mimeType': 'application/json', 'buffer': b'{}'})
            expect(page.get_by_test_id('simulation-status')).to_contain_text('unavailable')
            expect(page.get_by_test_id('simulation-hash')).to_be_empty()
            page.get_by_label('Load simulation archive').set_input_files(path)
            expect(page.get_by_test_id('simulation-status')).to_contain_text('All world hashes match')
            page.get_by_label('Load simulation archive').set_input_files({'name': 'large.json', 'mimeType': 'application/json', 'buffer': b' ' * (16 * 1024 * 1024 + 1)})
            expect(page.get_by_test_id('simulation-status')).to_contain_text('unavailable')
            expect(page.get_by_test_id('files')).to_be_empty()
            page.get_by_label('Load simulation archive').set_input_files(path)
            expect(page.get_by_test_id('simulation-status')).to_contain_text('All world hashes match')
            proof = Path(os.environ.get('VISUALIZER_PROOF_DIR', tmp_path))
            proof.mkdir(parents=True, exist_ok=True)
            for width in (1440, 390):
                page.set_viewport_size({'width': width, 'height': 1000})
                assert page.evaluate('document.documentElement.scrollWidth <= innerWidth')
                page.screenshot(path=str(proof / f'simulated-{width}.png'), full_page=True)
            delayed = []
            page.route('**/api/simulate', lambda route: delayed.append(route))
            page.get_by_role('button', name='Replay recorded inputs', exact=True).click()
            expect(page.get_by_test_id('simulation-status')).to_contain_text('Re-executing')
            expect(page.get_by_test_id('files')).to_be_empty()
            assert len(delayed) == 1
            page.get_by_role('button', name='Follow live', exact=True).click()
            response = delayed[0].fetch()
            delayed[0].fulfill(response=response)
            expect(page.get_by_test_id('connection')).to_contain_text('Observation unavailable')
            expect(page.get_by_test_id('simulation-hash')).to_be_empty()
            expect(page.get_by_test_id('hash')).to_be_empty()
            assert errors == []
            browser.close()
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
