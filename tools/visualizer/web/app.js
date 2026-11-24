'use strict';
const q = s => document.querySelector(s);
const token = location.hash.slice(1);
history.replaceState(null, '', location.pathname);
const decode = hex => new TextDecoder('utf-8', {fatal: true}).decode(Uint8Array.from(hex.match(/../g) || [], b => parseInt(b, 16)));
let snapshot = {records: []}, selected = null, pinned = null, signature = '';
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
  q('[data-testid=mode]').textContent = pinned ? `History · step ${record.step}` : 'Following latest export';
  if (!record) return;
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
  try {
    const reply = await fetch('/api/snapshot', {headers: {Authorization: `Bearer ${token}`}, cache: 'no-store'});
    if (!reply.ok) throw new Error(reply.status === 403 ? 'Authorization required; reopen the private launch URL' : `HTTP ${reply.status}`);
    const data = await reply.json();
    if (data.state === 'error') throw new Error(data.error);
    render(data);
  } catch (error) {
    q('[data-testid=connection]').textContent = `Observation unavailable · ${error.message}`;
    snapshot = {records: []}; signature = ''; selected = null; pinned = null;
    q('[data-testid=coglets]').replaceChildren(); clearDetail();
    q('#updated').textContent = ''; q('#provenance').textContent = 'Previous observations hidden. No current producer observation.';
  }
  setTimeout(poll, 500);
}
q('#filter').addEventListener('input', draw);
q('#live').onclick = () => { pinned = null; draw(); };
q('#help').onclick = () => q('dialog').showModal();
q('#close-help').onclick = () => q('dialog').close();
document.addEventListener('keydown', event => {
  const editing = ['INPUT', 'TEXTAREA'].includes(document.activeElement.tagName);
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
