import json
import os
from pathlib import Path
import subprocess
import sys
import threading

import cbor2
import pytest
from playwright.sync_api import sync_playwright, expect
from test_stream import load

NATIVE_TRACE = Path(__file__).parent / "fixtures" / "native-cli-wamr-trace.json"
NATIVE_PROVENANCE = Path(__file__).parent / "fixtures" / "native-cli-wamr-trace.provenance.json"

ROOT = Path(__file__).resolve().parents[3]


class Runtime:
    """Actual rc1 reference engine subprocess; no expected outputs injected."""
    def __init__(self, export):
        binary = Path(os.environ["COGLETD"]).resolve()
        self.process = subprocess.Popen([sys.executable, str(Path(__file__).parents[1] / 'observe.py'),
                                         '--records', str(export), '--cogletd', str(binary)],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        assert self.process.stdin is not None and self.process.stdout is not None
        self.export = export
        self.request = json.loads((ROOT / "conformance/abi/cases.json").read_text())[0]["steps"][0]["send"]

    def step(self, *, world=None):
        request = self.request
        if world is not None:
            request["guest_script"]["writes"] = {"/data/world.json": json.dumps(world).encode().hex()}
        payload = cbor2.dumps(request, canonical=True)
        self.process.stdin.write(len(payload).to_bytes(4, "big") + payload)
        self.process.stdin.flush()
        def read():
            header = self.process.stdout.read(4)
            assert len(header) == 4, "cogletd exited before reply"
            return cbor2.loads(self.process.stdout.read(int.from_bytes(header, "big")))
        response = read()
        read()  # The documented hash-only second response, not another journal.
        record = response["journal"]
        assert json.loads(self.export.read_text().splitlines()[-1]) == record
        request["inputs"]["step"] += 1
        request["inputs"]["logical_time"] += 10
        request["inputs"]["previous"] = record["record_hash"]
        request["inputs"]["events"][0]["seq"] += 1
        request["inputs"]["events"][0]["logical_time"] += 10
        return record

    def close(self):
        self.process.stdin.close()
        self.process.wait(timeout=5)
        self.process.stdout.close()


@pytest.fixture
def live(tmp_path):
    export = tmp_path / "journal.jsonl"
    server = load().make_server(export, port=0, world_path="/data/world.json")
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    runtime = Runtime(export)
    with sync_playwright() as pw:
        browser = pw.chromium.launch()
        page = browser.new_page(viewport={"width": 1440, "height": 1000})
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        try:
            yield page, server, runtime, errors
        finally:
            browser.close()
            runtime.close()
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


def test_live_cogletd_events_and_state_render(live):
    page, server, runtime, errors = live
    page.goto(server.url)
    expect(page.get_by_test_id("connection")).to_have_text("Waiting for exported records")
    record = runtime.step()
    expect(page.get_by_test_id("coglets")).to_contain_text(record["coglet"])
    expect(page.get_by_test_id("events")).to_contain_text("runtime.debug_step")
    expect(page.get_by_test_id("files")).to_contain_text("/data/count")
    expect(page.get_by_test_id("files")).to_contain_text("0")
    expect(page.get_by_test_id("hash")).to_have_text(record["output_hash"])
    assert errors == []


@pytest.mark.parametrize("world", [
    {"kind": "gridbot", "width": 8, "height": 6, "robot": {"x": 2, "y": 3}, "goal": {"x": 6, "y": 1}, "walls": [[4, 2]]},
    {"kind": "cartpole", "x": 0.25, "theta": 0.15},
])
def test_world_observation_and_history_are_live(live, world):
    page, server, runtime, errors = live
    first = runtime.step(world=world)
    page.goto(server.url)
    expect(page.get_by_test_id("world")).to_have_attribute("data-kind", world["kind"])
    expect(page.get_by_test_id("world").locator("svg")).to_be_visible()
    second = runtime.step(world=world)
    expect(page.get_by_role('button', name='Step 1', exact=True)).to_be_visible()
    expect(page.get_by_test_id("hash")).to_have_text(second["output_hash"])
    page.get_by_role("button", name="Step 0").click()
    expect(page.get_by_test_id("hash")).to_have_text(first["output_hash"])
    expect(page.get_by_test_id("mode")).to_have_text("History · step 0")
    page.get_by_role("button", name="Follow live").click()
    expect(page.get_by_test_id("mode")).to_have_text("Following latest export")
    assert errors == []
    if os.environ.get('VISUALIZER_PROOF_DIR'):
        proof = Path(os.environ['VISUALIZER_PROOF_DIR']); proof.mkdir(parents=True, exist_ok=True)
        page.screenshot(path=str(proof / (world['kind'] + '-desktop.png')), full_page=True)
        page.set_viewport_size({'width': 390, 'height': 844})
        assert page.evaluate('document.documentElement.scrollWidth <= innerWidth')
        page.screenshot(path=str(proof / (world['kind'] + '-mobile.png')), full_page=True)


def test_filter_keyboard_error_and_recovery(live):
    page, server, runtime, errors = live
    runtime.step()
    page.goto(server.url)
    expect(page.get_by_test_id("events")).to_contain_text("runtime.debug_step")
    page.keyboard.press("/")
    expect(page.locator("#filter")).to_be_focused()
    page.keyboard.type("not-present")
    expect(page.get_by_test_id("coglets")).to_contain_text("No matching coglets")
    page.keyboard.press("Escape")
    expect(page.get_by_test_id("coglets")).to_contain_text("coglet:")
    page.keyboard.press("?")
    expect(page.get_by_role("dialog")).to_be_visible()
    page.keyboard.press("Escape")
    parked = runtime.export.with_suffix('.parked')
    runtime.export.rename(parked)
    expect(page.get_by_test_id("connection")).to_contain_text("Observation unavailable")
    expect(page.get_by_test_id("files")).to_be_empty()
    parked.rename(runtime.export)
    runtime.step()
    expect(page.get_by_test_id("connection")).to_contain_text("Export available")
    expect(page.get_by_test_id("events")).to_contain_text("runtime.debug_step")
    assert errors == []


def test_real_inbox_boundary_and_surface_source_are_inspectable(live):
    page, server, runtime, errors = live
    cases = json.loads((ROOT / 'conformance/abi/cases.json').read_text())
    runtime.request = next(case for case in cases if case['name'] == 'inbox-boundary')['steps'][0]['send']
    runtime.step()
    page.goto(server.url)
    expect(page.get_by_test_id('inbox')).to_contain_text('hello')
    expect(page.get_by_test_id('events')).to_contain_text('runtime.inbox')
    assert errors == []


def test_uint64_history_and_empty_replacement_keep_exact_state(live):
    from test_stream import record
    page, server, runtime, errors = live
    runtime.step()
    values = [record(9007199254740992), record(9007199254740993)]
    values[0]['output_hash'] = 'b' * 64
    values[1]['output_hash'] = 'c' * 64
    runtime.export.write_text(''.join(json.dumps(value) + '\n' for value in values))
    page.goto(server.url)
    expect(page.get_by_role('button', name='Step 9007199254740993', exact=True)).to_be_visible()
    page.get_by_role('button', name='Step 9007199254740993', exact=True).click()
    expect(page.get_by_test_id('hash')).to_have_text('c' * 64)
    assert page.get_by_test_id('timeline').locator('[aria-pressed=true]').count() == 1
    runtime.export.write_text('')
    expect(page.get_by_test_id('connection')).to_have_text('Waiting for exported records')
    expect(page.locator('#provenance')).not_to_contain_text('55555555')
    expect(page.get_by_test_id('files')).to_be_empty()
    assert errors == []


def test_native_cli_wamr_trace_events_state_hash_and_history():
    """Actual child-WAMR CLI trace, not the rc1 observer and not a live hub."""
    provenance = json.loads(NATIVE_PROVENANCE.read_text())
    assert provenance["producer_sha"] == "866ede9b1ee7a1c20cc6e4afed723bfcb093ee4d"
    assert provenance["producer_pr"] == "https://github.com/softmax-universe/coglet/pull/82"
    assert provenance["engine"] == "wamr"
    trace = json.loads(NATIVE_TRACE.read_text())
    assert "initial_files" not in trace and "blobs" not in trace
    records = trace["records"]
    assert len(records) == 2
    assert records[0]["output_hash"] != records[1]["output_hash"]
    assert any(event["kind"] == "runtime.debug_step" for event in records[0]["events"])
    snapshot = load().RecordStream(NATIVE_TRACE, source_format="trace").snapshot()
    assert snapshot["records"][-1]["output_hash"] == records[1]["output_hash"]
    assert all(not path.startswith("/code") for record in snapshot["records"] for path in record["output_files"])
    server = load().make_server(NATIVE_TRACE, port=0, source_format="trace")
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with sync_playwright() as pw:
            browser = pw.chromium.launch()
            page = browser.new_page(viewport={"width": 1440, "height": 1000})
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(server.url)
            expect(page.get_by_test_id("connection")).to_contain_text("runtime readiness unknown")
            expect(page.get_by_test_id("coglets")).to_contain_text(records[0]["coglet"])
            expect(page.get_by_test_id("events")).to_contain_text("runtime.debug_step")
            expect(page.get_by_test_id("hash")).to_have_text(records[1]["output_hash"])
            expect(page.get_by_role("button", name="Step 0", exact=True)).to_be_visible()
            page.get_by_role("button", name="Step 0", exact=True).click()
            expect(page.get_by_test_id("hash")).to_have_text(records[0]["output_hash"])
            expect(page.get_by_test_id("mode")).to_have_text("History · step 0")
            expect(page.get_by_test_id("files")).not_to_contain_text("/code")
            expect(page.locator("#provenance")).to_contain_text("not independent replay verification")
            assert errors == []
            browser.close()
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def test_surface_observation_and_untrusted_text_render_as_text(live):
    page, server, runtime, errors = live
    event = runtime.request['inputs']['events'][0]
    event['source'] = 'surface:' + '2' * 64
    event['kind'] = 'sensor.tick'
    event['payload'] = {'text': '<img src=x onerror="window.XSS=true">'}
    runtime.step()
    page.goto(server.url)
    expect(page.get_by_test_id('surfaces')).to_contain_text(event['source'])
    assert json.loads(page.get_by_test_id('events').inner_text())['payload'] == event['payload']
    assert page.get_by_test_id('events').locator('img').count() == 0
    assert page.evaluate('window.XSS === undefined')
    assert errors == []
