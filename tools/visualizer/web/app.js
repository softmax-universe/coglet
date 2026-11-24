'use strict';
const q = s => document.querySelector(s);
const token = location.hash.slice(1);
history.replaceState(null, '', location.pathname);
const decode = hex => new TextDecoder('utf-8', {fatal: true}).decode(Uint8Array.from(hex.match(/../g) || [], b => parseInt(b, 16)));
let snapshot = {records: []}, selected = null, pinned = null, signature = '';
let simulation = null, simulationRaw = null, originalActions = [], worldIndex = 0, generation = 0, offline = false;
function clearSimulation(message) {
  simulation = null;
  q('#simulation-controls').hidden = true;
  for (const name of ['simulation-hash', 'expected-world-hash', 'simulation-state', 'simulation-step']) q(`[data-testid=${name}]`).textContent = '';
  q('[data-testid=simulation-status]').textContent = message;
}
function drawSimulation() {
  if (!simulation) return;
  const frames = simulation.frames;
  worldIndex = Math.min(Math.max(0, worldIndex), frames.length - 1);
  const frame = frames[worldIndex];
  q('#simulation-controls').hidden = false;
  q('#world-step').max = String(frames.length - 1);
  q('#world-step').value = String(worldIndex);
  q('[data-testid=simulation-step]').textContent = `World step ${worldIndex} / ${frames.length - 1}`;
  q('[data-testid=simulation-hash]').textContent = frame.hash;
  q('[data-testid=expected-world-hash]').textContent = frame.expected_hash;
  q('[data-testid=simulation-state]').textContent = JSON.stringify(frame.state, null, 2);
  q('#previous-world').disabled = worldIndex === 0;
  q('#next-world').disabled = worldIndex === frames.length - 1;
  q('#world-action').disabled = worldIndex >= originalActions.length;
  q('#run-what-if').disabled = worldIndex >= originalActions.length;
  q('#world-action').replaceChildren();
  const choices = simulation.world === 'gridbot-v1' ? ['north', 'south', 'east', 'west'] : ['left', 'right'];
  for (const action of choices) { const option = document.createElement('option'); option.value = action; option.textContent = action; q('#world-action').append(option); }
  q('#world-action').value = originalActions[worldIndex] || choices[0];
}
async function runSimulation(actions = null, ticket = ++generation) {
  clearSimulation('Re-executing offline world…');
  snapshot = {records: []}; signature = ''; selected = null; pinned = null;
  q('[data-testid=coglets]').replaceChildren(); clearDetail();
  q('#provenance').textContent = 'Offline replay pending; no current result.';
  q('[data-testid=connection]').textContent = 'Offline replay pending';
  q('#updated').textContent = '';
  try {
    const response = await fetch('/api/simulate', {method: 'POST', headers: {Authorization: `Bearer ${token}`, 'Content-Type': 'application/json'},
      body: `{"archive":${simulationRaw},"actions":${JSON.stringify(actions)}}`});
    if (ticket !== generation) return;
    if (!response.ok) throw new Error('Invalid or incomplete archive / input');
    const result = await response.json();
    if (ticket !== generation) return;
    simulation = result.simulation;
    if (actions === null) originalActions = [...simulation.actions];
    worldIndex = simulation.frames.length - 1;
    render(result);
    q('[data-testid=connection]').textContent = 'Offline archive · world-model replay only';
    q('[data-testid=simulation-status]').textContent = simulation.first_divergence === null ? 'All world hashes match recorded observations' : `First divergence: world step ${simulation.first_divergence}`;
    drawSimulation();
  } catch (error) {
    if (ticket !== generation) return;
    clearSimulation(`Simulation unavailable · ${error.message}`);
    snapshot = {records: []}; signature = ''; selected = null; pinned = null;
    q('[data-testid=coglets]').replaceChildren(); clearDetail();
    q('[data-testid=connection]').textContent = 'Offline simulation unavailable';
    q('#provenance').textContent = 'Previous observations hidden. Load a valid archive or Follow live.';
    q('#updated').textContent = '';
  }
}
q('#simulation-file').onchange = async () => {
  const ticket = ++generation;
  offline = true;
  simulationRaw = null;
  clearSimulation('Reading private archive…');
  snapshot = {records: []}; signature = ''; selected = null; pinned = null;
  q('[data-testid=coglets]').replaceChildren(); clearDetail();
  q('#provenance').textContent = 'Previous observations hidden. Reading a private archive.';
  q('#updated').textContent = '';
  q('[data-testid=connection]').textContent = 'Offline archive loading';
  const file = q('#simulation-file').files[0];
  if (!file || file.size > 16 * 1024 * 1024) {
    clearSimulation('Simulation unavailable · select a JSON archive up to 16 MiB');
    return;
  }
  try {
    const raw = await file.text();
    if (ticket !== generation) return;
    simulationRaw = raw;
    await runSimulation(null, ticket);
  } catch (error) { if (ticket === generation) clearSimulation(`Simulation unavailable · ${error.message}`); }
};
q('#world-step').oninput = () => { worldIndex = Number(q('#world-step').value); drawSimulation(); };
q('#previous-world').onclick = () => { worldIndex--; drawSimulation(); };
q('#next-world').onclick = () => { worldIndex++; drawSimulation(); };
q('#replay-inputs').onclick = () => runSimulation();
q('#run-what-if').onclick = () => {
  const actions = [...originalActions]; actions[worldIndex] = q('#world-action').value;
  runSimulation(actions);
};
const key = r => `${r.coglet}/${r.trace_id}/${r.step}`;
function line(parent, text, tag = 'pre') { const node = document.createElement(tag); node.textContent = text; parent.append(node); return node; }
function button(parent, label, action) { const node = line(parent, label, 'button'); node.type = 'button'; node.onclick = action; return node; }
function clearDetail() {
  for (const name of ['files', 'events', 'hash', 'world', 'inbox', 'surfaces', 'timeline']) q(`[data-testid=${name}]`).replaceChildren();
  q('#step').textContent = '';
}
function draw() {
  const data = snapshot;
  const term = q('#filter').value.toLowerCase();
  const ids = [...new Set(data.records.map(r => r.coglet))];
  if (!ids.includes(selected)) { selected = ids[0]; pinned = null; }
  const matches = ids.filter(id => data.records.some(r => r.coglet === id && JSON.stringify(r).toLowerCase().includes(term)));
  const nav = q('[data-testid=coglets]'); nav.replaceChildren();
  for (const id of matches) {
    const node = button(nav, id, () => { selected = id; pinned = null; draw(); });
    node.setAttribute('aria-pressed', String(selected === id));
  }
  if (!matches.length) line(nav, ids.length ? 'No matching coglets' : 'No exported coglets yet', 'p');
  clearDetail();
  const records = data.records.filter(r => r.coglet === selected);
  if (pinned && !records.some(r => key(r) === pinned)) pinned = null;
  const record = records.find(r => key(r) === pinned) || records.at(-1);
  q('[data-testid=mode]').textContent = offline ? (pinned ? `Recorded guest history · step ${record.step}` : 'Recorded guest archive · latest step') : (pinned ? `History · step ${record.step}` : 'Following latest export');
  if (!record) { q('#provenance').textContent = 'Waiting for a complete exported record. Runtime readiness is unknown.'; return; }
  q('#step').textContent = `Step ${record.step} · ${record.status}`;
  q('#provenance').textContent = `Trace ${record.trace_id} · logical time ${record.logical_time} · exported record; not independent replay verification`;
  q('[data-testid=hash]').textContent = record.output_hash;
  const timeline = q('[data-testid=timeline]');
  for (const item of records) {
    const node = button(timeline, `Step ${item.step}`, () => { pinned = key(item); draw(); });
    node.setAttribute('aria-pressed', String(key(item) === key(record)));
  }
  const files = q('[data-testid=files]');
  for (const [path, hex] of Object.entries(record.output_files)) {
    if (!path.startsWith('/data/')) continue;
    line(files, path, 'h3');
    try { line(files, decode(hex)); } catch { line(files, `[binary: ${hex.length / 2} bytes]`); }
  }
  const inbox = record.output_files['/data/inbox.jsonl'];
  if (inbox) { try { line(q('[data-testid=inbox]'), decode(inbox)); } catch { line(q('[data-testid=inbox]'), 'Binary inbox export'); } }
  else line(q('[data-testid=inbox]'), 'No inbox file in this exported snapshot', 'p');
  const events = q('[data-testid=events]');
  for (const event of record.events) line(events, JSON.stringify(event, null, 2));
  if (!record.events.length) line(events, 'No events in this step', 'p');
  const sources = [...new Set(record.events.map(e => e.source).filter(s => s.startsWith('surface:')))];
  for (const source of sources) line(q('[data-testid=surfaces]'), source);
  if (!sources.length) line(q('[data-testid=surfaces]'), 'No surface sources observed in this step. Registry/health unknown.', 'p');
  renderWorld(q('[data-testid=world]'), record.output_files[data.world_path]);
}
function render(data) {
  snapshot = data;
  q('[data-testid=connection]').textContent = data.records.length ? 'Export available · runtime readiness unknown' : 'Waiting for exported records';
  q('#updated').textContent = data.modified_at ? `Export modified ${new Date(data.modified_at * 1000).toLocaleString()}` : '';
  const next = JSON.stringify(data);
  if (next !== signature) { signature = next; draw(); }
}
async function poll() {
  const ticket = generation;
  if (offline) { setTimeout(poll, 500); return; }
  try {
    const reply = await fetch('/api/snapshot', {headers: {Authorization: `Bearer ${token}`}, cache: 'no-store'});
    if (!reply.ok) throw new Error(reply.status === 403 ? 'Authorization required; reopen the private launch URL' : `HTTP ${reply.status}`);
    const data = await reply.json();
    if (offline || ticket !== generation) { setTimeout(poll, 500); return; }
    if (data.state === 'error') throw new Error(data.error);
    render(data);
  } catch (error) {
    if (offline || ticket !== generation) { setTimeout(poll, 500); return; }
    q('[data-testid=connection]').textContent = `Observation unavailable · ${error.message}`;
    snapshot = {records: []}; signature = ''; selected = null; pinned = null;
    q('[data-testid=coglets]').replaceChildren(); clearDetail();
    q('#updated').textContent = ''; q('#provenance').textContent = 'Previous observations hidden. No current producer observation.';
  }
  setTimeout(poll, 500);
}
q('#filter').addEventListener('input', draw);
q('#live').onclick = () => {
  if (offline) {
    generation++; offline = false; simulationRaw = null; originalActions = [];
    clearSimulation('Select a private M2 world replay archive.');
    q('#simulation-file').value = '';
    snapshot = {records: []}; signature = ''; selected = null;
    q('[data-testid=coglets]').replaceChildren(); clearDetail();
    q('#provenance').textContent = 'Waiting for a current producer export.';
    q('[data-testid=connection]').textContent = 'Connecting';
  }
  pinned = null; draw();
};
q('#help').onclick = () => q('dialog').showModal();
q('#close-help').onclick = () => q('dialog').close();
document.addEventListener('keydown', event => {
  const editing = ['INPUT', 'TEXTAREA', 'SELECT'].includes(document.activeElement.tagName);
  if (event.key === 'Escape') { q('dialog').close(); q('#filter').value = ''; q('#filter').blur(); draw(); }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k' || (!editing && event.key === '/')) { event.preventDefault(); q('#filter').focus(); }
  else if (!editing && event.key === '?') { event.preventDefault(); q('dialog').showModal(); }
  else if (document.activeElement === q('#filter') && ['ArrowDown', 'Enter'].includes(event.key)) { event.preventDefault(); q('[data-testid=coglets] button')?.focus(); }
  else if (document.activeElement.closest('[data-testid=coglets]') && ['ArrowDown', 'ArrowUp'].includes(event.key)) {
    event.preventDefault(); const nodes = [...q('[data-testid=coglets]').querySelectorAll('button')];
    nodes[(nodes.indexOf(document.activeElement) + (event.key === 'ArrowDown' ? 1 : nodes.length - 1)) % nodes.length]?.focus();
  } else if (!editing && event.key.length === 1 && !event.ctrlKey && !event.metaKey && !q('dialog').open) { q('#filter').focus(); }
});
poll();
